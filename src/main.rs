use chat_with_tom::{
    CONFIG_FILE_PATH,
    config::{addresses_from_file, split_from_id},
    peer::PeerState,
};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    id: u32,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let (own_address, peers_indexes_addresses) =
        split_from_id(args.id, addresses_from_file(CONFIG_FILE_PATH)?);

    let state = PeerState::new(args.id, own_address)?;

    state.run(peers_indexes_addresses).await?;

    Ok(())
}
