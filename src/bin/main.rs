


use std::env;
use std::error::Error;
use elmo_trainer::ConfigElmo;
use elmo_trainer::ELMoText;
use elmo_trainer::Loader;
use elmo_trainer::Splitter;
use elmo_trainer::files_handling;
use elmo_trainer::Preprocessor;
use elmo_trainer::DatasetBuilder;
use elmo_trainer::training::ElmoTrainer;
use elmo_trainer::ELMo;
use tch::Tensor;
use tch::nn;


fn main() -> Result<(), Box<dyn Error>> {
    
    //
    // loading training parameteres
    println!("entering program...");
    let args: Vec<String> = env::args().collect();
    
    println!("building parameters...");
    let mut params = match ConfigElmo::new(&args) {
        Ok(config) => config.get_params(),
        Err(e) => panic!("{}", e)
    };
    println!("{}", params);
    // -- end of loading parameters --
    //

    //
    // preprocess of sentences
    let corpus_file = params.corpus_file.clone().unwrap();
    let mut sentences = files_handling::load_sentences(&corpus_file)?;
    let mut preprocessor = Preprocessor::new();
    let (token2int,char2int) = preprocessor.preprocess(&mut sentences, &mut params);
    // -- end of preprocessing sentences
    //

    //
    // Create an ELMo textual loader - data builder that moves data from strings to ints
    let n_samples = (&sentences).len() as i64;
    let elmo_text_loader = ELMoText::new(sentences, token2int, char2int, &params);
    // -- end of data building --
    //

    //
    // Create an instance of the ELMo model
    let mut vars = nn::VarStore::new(params.device);
    let model = ELMo::new(&vars.root(), &params);
    // -- end of instantiating model --
    //

    //
    // spliting data to train, dev and test sets, and moving to loaders (iterators over examples)
    let splitter = Splitter::new();
    let splits: Vec<Tensor> = splitter.get_split_train_dev_test_indices(n_samples);
    let mut iters = splits.iter().map(|split| {
        
        let indices = TryInto::<Vec<i64>>::try_into(split).unwrap();
        let (xs, ys): (Vec<_>, Vec<_>) = indices.iter()
        .map(|i| elmo_text_loader.get_example(*i as usize).unwrap())
        .unzip();

        Loader::new(xs, ys, params.device, params.batch_size, params.seq_length)
    });
    // -- end of creating train, dev, test iterators
    //
    
    //
    // running the training process with train and dev iterators
    let mut trainset_iter = iters.next().ok_or("iters is but should have multiple loaders empty")?;
    let mut devset_iter = iters.next();
    let elmo_train = ElmoTrainer::new();
    if let Err(e) = elmo_train.run_training(&mut trainset_iter, &mut devset_iter, &model, &mut vars, &params) {
        panic!("problem during training: {}", e)
    };
    // -- end of training process --
    //

    // 
    // do testing on test set with saved model
    if params.output_file.is_some() {

        vars.load(&params.output_file.unwrap().as_str())?;
        let mut testset_iter = iters.next().ok_or("iters doesn't have a testing loader but testing is called")?;
        assert!(iters.next().is_none());
    
        let test_acc = elmo_train.run_testing(&mut testset_iter, &model)?;
        println!("got {} acc on test set", test_acc);
        // -- end of testing --
        //

    }


    Ok(())
}   