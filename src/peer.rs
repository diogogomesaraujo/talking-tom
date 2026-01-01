use crate::peer::pb::peer_service_client::PeerServiceClient;
use crate::peer::pb::peer_service_server::{PeerService, PeerServiceServer};
use crate::peer::pb::{Clock, Message, RequestStatus};
use crate::poisson::Poisson;
use crate::{MALICIOUS_RATE, MAX_DIFF, RATE, WORDS_FILE_PATH, log};
use color_print::cformat;
use rand::{Rng, RngCore, rng};
use std::cmp::max;
use std::collections::HashSet;
use std::error::Error;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::process::exit;
use std::str::FromStr;
use std::time::Duration;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Notify, RwLock, mpsc};
use tokio::time::sleep;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

pub mod pb {
    tonic::include_proto!("peer");
}

#[derive(Debug)]
pub struct Connections {
    pub peers: HashMap<u32, mpsc::Sender<Message>>,
}

impl Connections {
    pub fn new() -> Self {
        Self {
            peers: HashMap::new(),
        }
    }

    pub async fn broadcast(&self, message: Message) -> Result<(), Box<dyn Error>> {
        for (_, tx) in &self.peers {
            tx.send(message.clone()).await?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub enum Intentions {
    Benevolent,
    Malicious,
}

pub struct MessageExecutionQueue {
    pub t: Vec<Message>,
}

impl MessageExecutionQueue {
    pub fn new() -> Self {
        Self { t: Vec::new() }
    }

    pub fn ordered_push(&mut self, message: Message, push_notifier: &Arc<Notify>) {
        self.t.push(message);
        self.t.sort_by(|m_1, m_2| {
            let clock_1 = m_1.clock.unwrap();
            let clock_2 = m_2.clock.unwrap();
            (clock_2.timestamp, clock_2.sender_id).cmp(&(clock_1.timestamp, clock_1.sender_id))
        });

        push_notifier.notify_waiters();
    }

    pub fn pop(&mut self) -> Option<Message> {
        self.t.pop()
    }

    pub fn get_all(&self) -> Vec<Message> {
        self.t.clone()
    }
}

#[derive(Clone)]
pub struct PeerState {
    pub address: String,
    pub id: u32,
    pub intentions: Intentions,
    pub time: Arc<RwLock<u64>>,
    pub connections: Arc<RwLock<Connections>>,
    pub history: Arc<RwLock<HashMap<u32, u64>>>,
    pub message_execution_queue: Arc<RwLock<MessageExecutionQueue>>,
    pub words: Arc<RwLock<Vec<String>>>,
    pub push_notifier: Arc<Notify>,
}

impl PeerState {
    pub fn new(id: u32, address: String, intentions: Intentions) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            address,
            id,
            intentions,
            time: Arc::new(RwLock::new(0)),
            connections: Arc::new(RwLock::new(Connections::new())),
            message_execution_queue: Arc::new(RwLock::new(MessageExecutionQueue::new())),
            words: Arc::new(RwLock::new(Self::words_from_file(WORDS_FILE_PATH)?)),
            push_notifier: Arc::new(Notify::new()),
            history: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn run(
        self,
        peer_indexes_addresses: Vec<(u32, String)>,
    ) -> Result<(), Box<dyn Error>> {
        for (peer_index, peer_address) in peer_indexes_addresses {
            let connections = self.connections.clone();

            let peer_address = match peer_address.starts_with("http") {
                true => peer_address.clone(),
                false => format!("http://{}", peer_address),
            };

            tokio::spawn(async move {
                sleep(Duration::from_secs(4)).await;

                let mut client = match PeerServiceClient::connect(peer_address.clone()).await {
                    Ok(client) => client,
                    Err(e) => {
                        log::error(&cformat!(
                            "Couldn't connect to <bold>{}</bold>. - {e}",
                            peer_address
                        ));
                        return;
                    }
                };

                let (tx, mut rx) = mpsc::channel(1);

                {
                    connections.write().await.peers.insert(peer_index, tx);
                }

                loop {
                    tokio::select! {
                        Some(msg) = rx.recv() => {
                             let request = Request::new(msg);

                             if let Err(_) = client.send_message(request).await {
                                 log::error("Failed to send message to peer.");
                                 return;
                             }
                         }
                    }
                }
            });
        }

        let mut seed: [u8; 32] = [0u8; 32];
        rng().fill_bytes(&mut seed);

        let poisson_process = Arc::new(RwLock::new(Poisson::new(RATE, &mut seed)));

        {
            let id = self.id.clone();
            let time = self.time.clone();
            let words = self.words.clone();
            let connections = self.connections.clone();
            let message_execution_queue = self.message_execution_queue.clone();
            let push_notifier = self.push_notifier.clone();
            let poisson_process = poisson_process.clone();

            tokio::spawn(async move {
                loop {
                    sleep(Duration::from_secs_f32(
                        poisson_process.write().await.time_for_next_event(),
                    ))
                    .await;

                    let timestamp = {
                        let current_time = *time.read().await + 1;
                        *time.write().await = current_time;
                        current_time
                    };

                    let len = { words.read().await.len() };

                    let content = {
                        let i = poisson_process.write().await.rng.random_range(0..len);
                        words.read().await[i].clone()
                    };

                    let message = Message {
                        clock: Some(Clock {
                            sender_id: id,
                            timestamp,
                        }),
                        content,
                    };

                    {
                        message_execution_queue
                            .write()
                            .await
                            .ordered_push(message.clone(), &push_notifier);
                    }

                    if let Err(_) = connections.read().await.broadcast(message).await {
                        log::error("Couldn't broadcast the message.");
                        return;
                    }
                }
            });
        }

        {
            let message_execution_queue = self.message_execution_queue.clone();
            let connections = self.connections.clone();
            let push_notifier = self.push_notifier.clone();

            tokio::spawn(async move {
                loop {
                    push_notifier.notified().await;

                    let number_of_unique_peers =
                        async |message_execution_queue: &Arc<RwLock<MessageExecutionQueue>>| {
                            let set = message_execution_queue.read().await.get_all().iter().fold(
                                HashSet::new(),
                                |mut id_set, m| {
                                    let clock = m.clock.unwrap();
                                    id_set.insert(clock.sender_id);
                                    id_set
                                },
                            );

                            set.len()
                        };

                    let mut peers_count = number_of_unique_peers(&message_execution_queue).await;

                    loop {
                        if peers_count < connections.read().await.peers.len() + 1 {
                            break;
                        }

                        let message_to_execute = { message_execution_queue.write().await.pop() };
                        match message_to_execute {
                            Some(message_to_print) => {
                                log::info(&cformat!(
                                    "<bold>({},{}):</bold> {}",
                                    message_to_print.clock.unwrap().sender_id,
                                    message_to_print.clock.unwrap().timestamp,
                                    message_to_print.content
                                ));
                            }
                            None => break,
                        };

                        peers_count = number_of_unique_peers(&message_execution_queue).await;
                    }
                }
            });
        }

        let intentions = self.intentions.clone();

        if matches!(intentions, Intentions::Malicious) {
            let time = self.time.clone();
            let poisson_process = poisson_process.clone();

            tokio::spawn(async move {
                loop {
                    sleep(Duration::from_secs_f32(
                        poisson_process.write().await.time_for_next_event() * MALICIOUS_RATE,
                    ))
                    .await;

                    let tampering = poisson_process
                        .write()
                        .await
                        .rng
                        .random_range(0..*time.read().await);
                    let sum_or_sub = poisson_process.write().await.rng.random_range(0..=1);

                    let current_time = *time.read().await;
                    *time.write().await = match sum_or_sub {
                        0 => {
                            log::warning(&cformat!(
                                "Tampered with time value -> <bold>-{}</bold>.",
                                tampering
                            ));

                            current_time - tampering
                        }
                        _ => {
                            log::warning(&cformat!(
                                "Tampered with time value -> <bold>+{}</bold>.",
                                tampering
                            ));

                            current_time + tampering
                        }
                    };
                }
            });
        }

        let address = self.address.clone();

        Server::builder()
            .add_service(PeerServiceServer::new(self))
            .serve(SocketAddr::from_str(&address).unwrap())
            .await?;

        Ok(())
    }

    fn words_from_file(file_path: &str) -> Result<Vec<String>, Box<dyn Error>> {
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
}

#[tonic::async_trait]
impl PeerService for PeerState {
    async fn send_message(
        &self,
        request: Request<Message>,
    ) -> Result<Response<RequestStatus>, Status> {
        let push_notifier = self.push_notifier.clone();
        let history = self.history.clone();
        let intentions = self.intentions.clone();

        let received_message = request.into_inner();
        if let Some(clock) = &received_message.clock {
            {
                {
                    if let Some(last_timestamp) = history.read().await.get(&clock.sender_id) {
                        // a
                        if matches!(intentions, Intentions::Benevolent)
                            && clock.timestamp <= *last_timestamp
                        {
                            log::error(&cformat!(
                                "Declaring <bold>{}</bold> as malicious peer for reason <bold>(A)</bold> and aborting protocol.",
                                clock.sender_id
                            ));

                            exit(1);

                            // return Ok(Response::new(RequestStatus { v: 1 }));
                        }

                        // b
                        if matches!(intentions, Intentions::Benevolent)
                            && clock.timestamp > *self.time.clone().read().await + MAX_DIFF
                        {
                            log::error(&cformat!(
                                "Declaring <bold>{}</bold> as malicious peer for reason <bold>(B)</bold> and aborting protocol.",
                                clock.sender_id
                            ));

                            exit(1);

                            // return Ok(Response::new(RequestStatus { v: 1 }));
                        }
                    }

                    history
                        .write()
                        .await
                        .insert(clock.sender_id, clock.timestamp);
                }
                let current_time = *self.time.read().await;
                *self.time.write().await = max(current_time, clock.timestamp) + 1;
            }

            self.message_execution_queue
                .write()
                .await
                .ordered_push(received_message, &push_notifier);
        }

        Ok(Response::new(RequestStatus { v: 0 }))
    }
}
