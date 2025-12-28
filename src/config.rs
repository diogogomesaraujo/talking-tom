use std::{
    error::Error,
    fs::File,
    io::{BufRead, BufReader},
};

pub fn addresses_from_file(file_path: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let file = File::open(file_path)?;
    let reader = BufReader::new(&file);

    reader
        .lines()
        .into_iter()
        .try_fold(Vec::new(), |mut acc, line| {
            acc.push(line?);
            Ok(acc)
        })
}

pub fn split_from_id(id: u32, addresses: Vec<String>) -> (String, Vec<(u32, String)>) {
    let id_address = addresses[id as usize].clone();

    let peers_addresses = addresses
        .iter()
        .enumerate()
        .filter_map(|(i, a)| match i != id as usize {
            true => Some((i as u32, a.clone())),
            false => None,
        })
        .collect::<Vec<(u32, String)>>();

    (id_address, peers_addresses)
}
