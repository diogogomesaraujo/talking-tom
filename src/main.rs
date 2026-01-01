use chat_with_tom::{
    CONFIG_FILE_PATH,
    config::{addresses_from_file, split_from_id},
    peer::{Intentions, PeerState},
};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    id: u32,

    #[arg(long)]
    malicious: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let (own_address, peers_indexes_addresses) =
        split_from_id(args.id, addresses_from_file(CONFIG_FILE_PATH)?);

    let intentions = match args.malicious {
        true => Intentions::Malicious,
        false => Intentions::Benevolent,
    };

    let state = PeerState::new(args.id, own_address, intentions)?;

    state.run(peers_indexes_addresses).await?;

    Ok(())
}
