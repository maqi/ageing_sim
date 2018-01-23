extern crate clap;
#[macro_use]
extern crate log;
extern crate rand;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate tiny_keccak;

mod network;
mod random;
mod params;
mod stats;

use random::random_range;
use network::{Network, NetworkStructure};
use params::Params;
use std::collections::BTreeMap;
use clap::{App, Arg};

/// Generates a random churn event in the network. There are three possible kinds:
/// node joining, node leaving and node rejoining.
fn random_event(network: &mut Network, probs: (u8, u8)) {
    let x = random_range(0, 100);
    if x < probs.0 {
        network.add_random_node();
    } else if x >= probs.0 && x < probs.0 + probs.1 {
        network.drop_random_node();
    } else {
        network.rejoin_random_node();
    }
}

fn print_dist(mut dist: BTreeMap<u8, usize>) {
    let mut age = 1;
    while !dist.is_empty() {
        let num = dist.remove(&age).unwrap_or(0);
        println!("{}\t{}", age, num);
        age += 1;
    }
}

fn get_params() -> Params {
    let matches = App::new("Ageing Simulation")
        .about("Simulates ageing in SAFE network")
        .arg(
            Arg::with_name("initage")
                .short("i")
                .long("initage")
                .value_name("AGE")
                .help("Sets the initial age of newly joining peers; default: 1")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("split")
                .short("s")
                .long("split")
                .value_name("STRATEGY")
                .help("Selects the strategy for splitting (always/complete); default: complete")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("max_young")
                .short("y")
                .long("max_young")
                .value_name("MAX")
                .help("Set the max number of young peers we allow in a section; 0 value means no control; default: 1")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("iterations")
                .short("n")
                .long("iterations")
                .value_name("ITER")
                .help("Number of iterations; default: 100000")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("summary_intervals")
                .short("summary")
                .long("summary_intervals")
                .value_name("SUMMARY_INTERVALS")
                .help("Intervals of summary; default: 10000")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("p_add1")
                .long("padd1")
                .value_name("P")
                .help("Probability that a peer will join during a step (0-100); default: 90")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("p_drop1")
                .long("pdrop1")
                .value_name("P")
                .help("Probability that a peer will be dropped during a step (0-100); default: 7")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("struct_file")
                .long("network-struct-out")
                .short("f")
                .value_name("FILE")
                .help("Output file for network structure data")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("drop_dist")
                .long("drop-dist")
                .value_name("DISTR")
                .help("Drop probability distribution based on the age: exponential(exp)/reverse-proportional(rev) (default: exponential)")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("age_inc")
                .long("age-inc")
                .short("a")
                .help("Increment node ages on merges and splits")
        )
        .get_matches();
    let init_age = matches
        .value_of("initage")
        .unwrap_or("4")
        .parse()
        .expect("Initial age must be a number!");
    let split = matches
        .value_of("split")
        .unwrap_or("complete")
        .parse()
        .ok()
        .expect("Split strategy must be \"always\" or \"complete\".");
    let drop_dist = matches
        .value_of("drop_dist")
        .unwrap_or("exp")
        .parse()
        .ok()
        .expect("Drop distribution must be exp/exponential/rev/reverse-proportional.");
    let max_young = matches
        .value_of("max_young")
        .unwrap_or("1")
        .parse()
        .expect("Max number of young peers must be a number!");
    let iterations = matches
        .value_of("iterations")
        .unwrap_or("100000")
        .parse()
        .expect("Number of iterations must be a number!");
    let summary_intervals = matches
        .value_of("summary_intervals")
        .unwrap_or("10000")
        .parse()
        .expect("Number of summary intervals must be a number!");
    let inc_age = matches.is_present("age_inc");
    let p_add1 = matches
        .value_of("p_add1")
        .unwrap_or("90")
        .parse()
        .expect("Add probability must be a number!");
    assert!(p_add1 < 100, "Probability must be between 0 and 100!");
    let p_drop1 = matches
        .value_of("p_drop1")
        .unwrap_or("7")
        .parse()
        .expect("Drop probability must be a number!");
    assert!(p_drop1 < 100, "Probability must be between 0 and 100!");
    assert!(
        p_add1 + p_drop1 <= 100,
        "Add and drop probabilites must add up to at most 100!"
    );
    let structure_output_file = matches.value_of("struct_file").map(|s| s.to_owned());
    Params {
        init_age,
        split_strategy: split,
        max_young,
        iterations,
        summary_intervals,
        growth: (p_add1, p_drop1),
        structure_output_file,
        drop_dist,
        inc_age,
    }
}

fn output_structure_file(file: &str, data: &[NetworkStructure]) {
    use std::fs::File;
    use std::io::Write;
    let mut file = File::create(file)
        .ok()
        .expect(&format!("Couldn't create file {}!", file));
    for (i, data) in data.into_iter().enumerate() {
        let _ = write!(
            file,
            "{} {} {} {}\n",
            i, data.size, data.sections, data.complete
        );
    }
}

fn main() {
    let params = get_params();
    let mut network = Network::new(params.clone());

    for i in 0..params.iterations {
        if i % params.summary_intervals == 0 {
            println!("Iteration {}...", i);
            println!("Network state:\n{}", network);
            println!("");            
        }
        // Generate a random event...
        random_event(&mut network, params.growth);
        // ... and process the churn cascade that may happen
        // (every churn event may trigger other churn events, that
        // may trigger others etc.)
        network.process_events();
    }

    println!("...Iteration {}", params.iterations - 1);
    println!("Network state:\n{}", network);
    println!("");

    println!("{:?}\n", params.clone());

    let age_dist = network.age_distribution();
    println!("\nAge distribution:");
    print_dist(age_dist);

    let drop_dist = &network.output().drops_dist;
    println!("\nDrops distribution by age:");
    print_dist(drop_dist.clone());

    if let Some(ref file) = params.structure_output_file {
        output_structure_file(file, &network.output().network_structure);
    }
}
