
use std::iter::zip;
use std::ops::Mul;

use tch::{nn, Tensor, Device, Kind};
use tch::nn::{ModuleT, RNN};


// If a word is of length k charachters, the convolution is What's described as
// a narrow convolution between a charachter within a word, C_k \in (d, l),  
// and a kernel H \in (d, w), where d is char embedding, l is the length of the word,
// and w is the size of the kernal. The multiplication is done as a dot product.
#[derive(Debug)]
pub struct CnnBlock {
    conv: nn::Conv1D,
    kernel_size: i64
}

impl CnnBlock {
    fn new(vars: &nn::Path, in_channels: i64, out_channels: i64, kernel_size: i64) -> Self {

        // handling embedding dim d as input channels.
        // hadnling number of filters h as output channels.
        let conv = nn::conv1d(vars, in_channels, out_channels, kernel_size, Default::default());
        let kernel_size = kernel_size;

        Self {
            conv: conv,
            kernel_size: kernel_size
        }

    }
}

impl ModuleT for CnnBlock {
    fn forward_t(&self, xs: &Tensor, _train: bool) -> Tensor {
        
        // xs is of shape (batch_size, 1, token_length, embedding_dim)
        let dims = xs.internal_shape_as_tensor();
        let dims = Vec::<i64>::from(dims);
        assert!(4 == dims.len());

        let batch_size = dims[0];
        let token_length = dims[2];
        let embedding_dim = dims[3];
        let pool_kernel: i64 = token_length - self.kernel_size + 1;

        // xs reshape : (batch_size, token_length, embedding_dim) => (batch_size, embedding_dim, token_length)
        let xs = xs.reshape(&[batch_size, embedding_dim, token_length]);

        // denote token_length = l, out_channels = h = number of filters, kernel_size = w, embedding_dim = d
        // self.conv.w is of shape (h, d, w)
        // conv does (h,d,w) * (batch_size, d, l) => (batch_size, h, l-w+1)
        let conv_out = xs.apply(&self.conv);

        // tanh doesn't change dims, (batch_size, h, l-w+1)
        let act_out = conv_out.tanh();
         
         // max_pool1d moves xs : (batch_size, h, l-w+1) => (batch_size, h, 1)
        let pool_out = act_out.max_pool1d(&[pool_kernel], &[1], &[0], &[1], false);

        // the output should be (batch_size, h)
        let out = pool_out.squeeze_dim(2);

        
        out
    }
}

#[derive(Debug)]
struct Highway {
    w_t: nn::Linear,
    w_h: nn::Linear,
}

impl Highway {
    
    fn new(vars: &nn::Path, in_dim: i64, out_dim: i64) -> Self {

        let w_t = nn::linear(vars, in_dim, out_dim, Default::default());
        let w_h = nn::linear(vars, in_dim, out_dim, Default::default());
    
        Self {
            w_t: w_t,
            w_h: w_h
        }
    }

}

impl ModuleT for Highway {
    fn forward_t(&self, xs: &Tensor, _train: bool) -> Tensor {

        let t = xs.apply(&self.w_t).sigmoid();
        let transform_part = xs.apply(&self.w_h).relu().mul(&t);
        let carry_part = xs.mul(1-t);
        let out: Tensor = transform_part + carry_part;
        out

        // xs should remain (batch_size, total_filters) from input to end
    }
}

#[derive(Debug)]
pub struct CharLevelNet {
    embedding: nn::Embedding,
    conv_blocks: Vec<CnnBlock>,
    highways: Vec<Highway>,
    out_linear: nn::Linear
}

impl CharLevelNet {
    pub fn new(vars: &nn::Path,
         vocab_size: i64, 
         embedding_dim: i64, 
         in_channels: i64, 
         out_channels: Vec<i64>, 
         kernel_size: Vec<i64>, 
         highways: i64, 
         char_level_out_dim: i64) -> Self {

            // out_channels = number of filters
            // kernel_size = matching kernel width

        let embedding = nn::embedding(vars, vocab_size,  embedding_dim, Default::default());
        let mut conv_blocks = Vec::new();
        for (out_channel, kernel_size) in zip(&out_channels, kernel_size) {
            let conv_block = CnnBlock::new(vars, in_channels, *out_channel, kernel_size);
            conv_blocks.push(conv_block);
        }

        // total filters should be the sum over out_channels
        let total_filters: i64 = (&out_channels).iter().sum();
        let mut highway_layers = Vec::new();
        for _ in 0..highways {
            let highway = Highway::new(vars, total_filters, total_filters);
            highway_layers.push(highway);
        }

        // output to linear
        let out_linear = nn::linear(vars, total_filters, char_level_out_dim, Default::default());
        
        Self {
            embedding: embedding,
            conv_blocks: conv_blocks,
            highways: highway_layers,
            out_linear: out_linear
        }

    }
    

}

impl ModuleT for CharLevelNet {
    
    fn forward_t(&self, xs: &Tensor, train: bool) -> Tensor {

        // xs is of shape (batch_size, sequence_length, token_length), batch_size = 1
        let dims = xs.internal_shape_as_tensor();
        let dims = Vec::<i64>::from(dims);
        let batch_size = &dims[0];
        let seq_length = &dims[1];
        
        // iterate over tokens
        let mut outputs = Vec::new();
        for s in 0..*seq_length {

            let xs_tokens: Tensor = xs.slice(1, s, s+1, 1); // should be (batch_size, 1, token_length)
            let x = xs_tokens.apply(&self.embedding); // should be (batch_size, 1, token_length, embedding_dim)

            let mut token_outputs = Vec::new();
            for conv_block in &self.conv_blocks {
                let out = conv_block.forward_t(&x, train); // out is of shape (batch_size, n_filters)
                token_outputs.push(out);
            }

            // each output in token_outputs is of shape k * (batch_size, n_filters) => (batch_size, total_filters)
            let mut token_outputs = Tensor::concat(&token_outputs, 1);

            // move through highways, remains (batch_size, total_filters)
            for highway in &self.highways {
                token_outputs = highway.forward_t(&token_outputs, train);
            }

            outputs.push(token_outputs);
        }

        // (sequence_length, batch_size, total_filters) => (batch_size, sequence_length, total_filters)
        let outputs = Tensor::concat(&outputs, 0).reshape(&[*batch_size, *seq_length, -1]);

        // move to linear out (batch_size, sequence_length, total_filters) => (batch_size, sequence_length, out_linear)
        let outputs = outputs.apply(&self.out_linear);
        outputs


    }
}


#[derive(Debug)]
pub struct UniLM {
    lstm_layers: Vec<nn::LSTM>,
    to_rep: nn::Linear,
    hidden_dim: i64
}

impl UniLM {
    pub fn new(vars: &nn::Path, n_lstm_layers: i64, in_dim: i64, hidden_dim: i64, out_dim: i64) -> Self {

        let mut lstm_layers = Vec::new();
        for _ in 0..n_lstm_layers {
            let lm = nn::lstm(vars, in_dim, hidden_dim, Default::default());
            lstm_layers.push(lm);
        }

        let to_rep = nn::linear(vars, hidden_dim, out_dim, Default::default());

        Self {
            lstm_layers: lstm_layers,
            to_rep: to_rep,
            hidden_dim: hidden_dim
        }


    }
}

impl ModuleT for UniLM {

    fn forward_t(&self, xs: &Tensor, _train: bool) -> Tensor {
        
        // xs should be (batch_size, sequence_length, out_linear)
        let dims = xs.internal_shape_as_tensor();
        let dims = Vec::<i64>::from(dims);
        let batch_size = dims[0];

        let h = Tensor::zeros(&[batch_size, self.hidden_dim], (Kind::Int, Device::Cpu));
        let c = Tensor::zeros(&[batch_size, self.hidden_dim], (Kind::Int, Device::Cpu));
        let mut state = nn::LSTMState((h, c));
        
        // need residual connections, so lstm out should be the same size of input
        let mut out = xs.to_owned().shallow_clone();
        let mut outputs = vec![xs.to_owned().shallow_clone()];

        for (j, lstm) in (&self.lstm_layers).iter().enumerate() {
            
            let out_lstm = lstm.seq_init(&out, &state);
            out = out_lstm.0;
            state = out_lstm.1;
            
            // out moves (batch_size, sequence_length, hidden_dim) => (batch_size, sequence_length, out_linear)
            out = out.apply(&self.to_rep);

            outputs.push(out.shallow_clone());

            out += outputs[j].shallow_clone();

        }

        // move n_lstm_layers * (batch_size, sequence_length, out_linear) =>  (n_lstm_layers, batch_size, sequence_length, out_linear)
        let outputs = Tensor::concat(&outputs, 0);
        outputs

    }
}