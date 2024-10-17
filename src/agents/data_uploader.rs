//! Data uploader agent.
//!
//! The agent uploads data asynchronously in the background to the backend.

use crate::{backend, config::Config, dd_incr, dd_timing, ssd};
use agentwire::port::{self, Port};
use eyre::{Error, Result};
use futures::{channel::oneshot, prelude::*, stream::FuturesUnordered};
use orb_wld_data_id::SignupId;
use serde::{Deserialize, Serialize};
use std::{
    array,
    collections::{BTreeSet, VecDeque},
    convert::Infallible,
    mem::take,
    path::PathBuf,
    sync::Arc,
    time::Instant,
};
use tokio::{fs, select, sync::Mutex};

const PARALLEL_UPLOAD_STREAMS: usize = 4;
const TIERS_COUNT: u8 = 2;

/// Data uploader agent.
#[derive(Debug)]
pub struct Agent {
    /// Shared Orb configuration.
    pub config: Arc<Mutex<Config>>,
}

/// Data uploader agent input.
#[derive(Debug)]
pub enum Input {
    /// Push a personal-custody package to the upload queue.
    Pcp(Pcp),
    /// Wait for all queues to be not full.
    WaitQueues(oneshot::Sender<()>),
}

/// Personal-custody package to upload.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pcp {
    /// Signup ID.
    pub signup_id: SignupId,
    /// User ID.
    pub user_id: String,
    /// Package contents.
    #[serde(skip)]
    pub data: Vec<u8>,
    /// Package checksum.
    pub checksum: Vec<u8>,
    /// Package tier.
    pub tier: u8,
}

enum Queue {
    Memory {
        queue: VecDeque<Pcp>,
    },
    #[allow(dead_code)]
    Persistent {
        path: PathBuf,
        queue: VecDeque<u64>,
        next_id: u64,
        in_progress: u64,
    },
}

impl Port for Agent {
    type Input = Input;
    type Output = Infallible;

    const INPUT_CAPACITY: usize = 4;
    const OUTPUT_CAPACITY: usize = 0;
}

impl agentwire::Agent for Agent {
    const NAME: &'static str = "data-uploader";
}

macro_rules! log_queues {
    ($queues:ident) => {
        tracing::debug!(
            "Data Uploader queue sizes: tier 1: {}, tier 2: {}",
            $queues[0].len(),
            $queues[1].len()
        );
    };
}

impl agentwire::agent::Task for Agent {
    type Error = Error;

    async fn run(self, mut port: port::Inner<Self>) -> Result<(), Self::Error> {
        let Config {
            pcp_tier1_blocking_threshold,
            pcp_tier1_dropping_threshold,
            pcp_tier2_blocking_threshold,
            pcp_tier2_dropping_threshold,
            ..
        } = *self.config.lock().await;
        let blocking_thresholds = [pcp_tier1_blocking_threshold, pcp_tier2_blocking_threshold];
        let dropping_thresholds = [pcp_tier1_dropping_threshold, pcp_tier2_dropping_threshold];
        let mut queues: [_; TIERS_COUNT as usize] = array::from_fn(|_| Queue::new_memory());
        let mut uploaders = FuturesUnordered::new();
        let mut waiters = Vec::<oneshot::Sender<()>>::new();
        let check_blocking = |queues: &[Queue]| -> bool {
            for (i, queue) in queues.iter().enumerate() {
                if queue.len() >= blocking_thresholds[i] as usize {
                    return true;
                }
            }
            false
        };
        loop {
            select! {
                biased;
                Some((tier, id)) = uploaders.next() => {
                    queues[usize::from(tier - 1)].commit(id).await;
                    if !check_blocking(&queues) {
                        for tx in take(&mut waiters) {
                            tx.send(()).unwrap();
                        }
                    }
                    for queue in &mut queues {
                        if let Some((pcp, id)) = queue.pop().await {
                            log_queues!(queues);
                            uploaders.push(self.upload_pcp(pcp, id));
                            break;
                        }
                    }
                },
                input = port.next() => match input {
                    None => break,
                    Some(input) => match input.value {
                        Input::Pcp(pcp) => {
                            if !(1..=TIERS_COUNT).contains(&pcp.tier) {
                                tracing::error!("Invalid tier: {}", pcp.tier);
                                continue;
                            }
                            let i = usize::from(pcp.tier - 1);
                            if queues[i].len() == dropping_thresholds[i] as usize {
                                queues[i].drop_oldest().await;
                            }
                            queues[i].push(pcp).await;
                            if uploaders.len() < PARALLEL_UPLOAD_STREAMS {
                                if let Some((pcp, id)) = queues[i].pop().await {
                                    log_queues!(queues);
                                    uploaders.push(self.upload_pcp(pcp, id));
                                }
                            }
                        },
                        Input::WaitQueues(tx) => {
                            if check_blocking(&queues) {
                                waiters.push(tx);
                            } else {
                                tx.send(()).unwrap();
                            }
                        },
                    }
                },
            }
        }
        Ok(())
    }
}

impl Agent {
    async fn upload_pcp(&self, pcp: Pcp, id: u64) -> (u8, u64) {
        let Pcp { signup_id, user_id, data, checksum, tier } = pcp;
        tracing::info!(
            "Start uploading a personal custody package tier {tier} for signup_id={signup_id}"
        );
        let t = Instant::now();
        loop {
            let response = backend::upload_personal_custody_package::request(
                &signup_id,
                &user_id,
                checksum.as_ref(),
                &data,
                Some(tier),
                &self.config,
            )
            .await;
            match response {
                Ok(()) => {
                    dd_timing!("main.time.signup.upload_custody_images" + format!("t{}", tier), t);
                    tracing::info!(
                        "Personal custody package tier {tier} uploading completed in: {}ms",
                        t.elapsed().as_millis()
                    );
                    break;
                }
                Err(err) => {
                    tracing::error!("UPLOAD PERSONAL CUSTODY PACKAGE TIER {tier} ERROR: {err:?}");
                    dd_incr!(
                        "main.count.http.upload_custody_images.error.network_error",
                        "error_type:normal"
                    );
                    if let Some(reqwest_err) = err.downcast_ref::<reqwest::Error>() {
                        if let Some(status) = reqwest_err.status() {
                            if status.is_client_error() {
                                dd_incr!(
                                    "main.count.signup.result.failure.upload_custody_images",
                                    "type:network_error",
                                    "subtype:signup_request"
                                );
                                break;
                            }
                        }
                    }
                }
            }
        }
        (tier, id)
    }
}

impl Queue {
    fn new_memory() -> Self {
        Self::Memory { queue: VecDeque::new() }
    }

    #[allow(dead_code)]
    async fn new(path: PathBuf) -> Self {
        let ssd_perform = ssd::perform_async(async {
            fs::create_dir_all(&path).await?;
            let mut ids = BTreeSet::new();
            let mut read_dir = fs::read_dir(&path).await?;
            while let Some(entry) = read_dir.next_entry().await? {
                if let Some(name) = entry.path().file_name() {
                    if let Ok(id) = name.to_string_lossy().parse::<u64>() {
                        assert!(ids.insert(id), "duplicate entry in data uploader directory");
                    } else {
                        tracing::error!(
                            "Data uploader directory contains a non-integer entry: {name:?}",
                        );
                        return Ok(None);
                    }
                }
            }
            Ok(Some(ids.into_iter().collect::<VecDeque<_>>()))
        });
        match ssd_perform.await {
            None | Some(None) => Self::new_memory(),
            Some(Some(queue)) => {
                let next_id =
                    queue.back().map_or(0, |id| id.checked_add(1).expect("shouldn't grow so fast"));
                Self::Persistent { path, queue, next_id, in_progress: 0 }
            }
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Memory { queue } => queue.len(),
            Self::Persistent { queue, .. } => queue.len(),
        }
    }

    async fn push(&mut self, pcp: Pcp) {
        match self {
            Self::Memory { queue } => {
                queue.push_back(pcp);
            }
            Self::Persistent { path, queue, next_id, .. } => {
                let ssd_perform = ssd::perform_async(async {
                    let path = path.join(next_id.to_string());
                    let meta = serde_json::to_string(&pcp)?;
                    fs::create_dir_all(&path).await?;
                    fs::write(path.join("meta.json"), meta).await?;
                    fs::write(path.join("data.bin"), &pcp.data).await?;
                    Ok(())
                });
                match ssd_perform.await {
                    None => {
                        tracing::error!(
                            "Persistent queue is failed during push, switching to memory"
                        );
                        *self = Self::Memory { queue: vec![pcp].into() };
                    }
                    Some(()) => {
                        queue.push_back(*next_id);
                        *next_id = next_id.checked_add(1).expect("shouldn't grow so fast");
                    }
                }
            }
        }
    }

    async fn pop(&mut self) -> Option<(Pcp, u64)> {
        match self {
            Self::Memory { queue } => queue.pop_front().map(|pcp| (pcp, 0)),
            Self::Persistent { path, queue, in_progress, .. } => {
                let id = queue.pop_front()?;
                let ssd_perform = ssd::perform_async(async {
                    let path = path.join(id.to_string());
                    let meta = fs::read_to_string(path.join("meta.json")).await?;
                    let mut pcp = serde_json::from_str::<Pcp>(&meta)?;
                    pcp.data = fs::read(path.join("data.bin")).await?;
                    Ok(pcp)
                });
                match ssd_perform.await {
                    None => {
                        tracing::error!(
                            "Persistent queue is failed during pop, switching to memory"
                        );
                        *self = Self::new_memory();
                        None
                    }
                    Some(pcp) => {
                        *in_progress += 1;
                        Some((pcp, id))
                    }
                }
            }
        }
    }

    async fn commit(&mut self, id: u64) {
        match self {
            Self::Memory { .. } => {}
            Self::Persistent { path, next_id, in_progress, .. } => {
                *in_progress = in_progress.checked_sub(1).expect("shouldn't go negative");
                match ssd::perform_async(fs::remove_dir_all(path.join(id.to_string()))).await {
                    None => {
                        tracing::error!(
                            "Persistent queue is failed during commit, switching to memory"
                        );
                        *self = Self::new_memory();
                    }
                    Some(()) => {
                        if *in_progress == 0 {
                            *next_id = 0;
                        }
                    }
                }
            }
        }
    }

    async fn drop_oldest(&mut self) {
        match self {
            Self::Memory { queue } => {
                queue.pop_back();
            }
            Self::Persistent { path, queue, .. } => {
                let Some(id) = queue.pop_back() else { return };
                match ssd::perform_async(fs::remove_dir_all(path.join(id.to_string()))).await {
                    None => {
                        tracing::error!(
                            "Persistent queue is failed during drop_oldest, switching to memory"
                        );
                        *self = Self::new_memory();
                    }
                    Some(()) => {}
                }
            }
        }
    }
}

/// Waits for all queues to be not full.
pub async fn wait_queues(port: &mut port::Outer<Agent>) -> Result<()> {
    let (tx, rx) = oneshot::channel();
    port.send(port::Input::new(Input::WaitQueues(tx))).await?;
    Ok(rx.await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_memory_queue() {
        let mut queue = Queue::new_memory();
        queue
            .push(Pcp {
                signup_id: SignupId::default(),
                user_id: "test".to_string(),
                data: vec![1, 2, 3],
                checksum: vec![4, 5, 6],
                tier: 0,
            })
            .await;
        queue
            .push(Pcp {
                signup_id: SignupId::default(),
                user_id: "test".to_string(),
                data: vec![7, 8, 9],
                checksum: vec![10, 11, 12],
                tier: 0,
            })
            .await;
        assert_eq!(
            queue.pop().await,
            Some((
                Pcp {
                    signup_id: SignupId::default(),
                    user_id: "test".to_string(),
                    data: vec![1, 2, 3],
                    checksum: vec![4, 5, 6],
                    tier: 0,
                },
                0
            ))
        );
        assert_eq!(
            queue.pop().await,
            Some((
                Pcp {
                    signup_id: SignupId::default(),
                    user_id: "test".to_string(),
                    data: vec![7, 8, 9],
                    checksum: vec![10, 11, 12],
                    tier: 0,
                },
                0
            ))
        );
        assert_eq!(queue.pop().await, None);
    }

    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn test_persistent_queue() {
        let tempdir = tempdir().unwrap();
        let mut queue = Queue::new(tempdir.path().to_path_buf()).await;
        assert!(matches!(queue, Queue::Persistent { .. }));
        queue
            .push(Pcp {
                signup_id: SignupId::default(),
                user_id: "test".to_string(),
                data: vec![1, 2, 3],
                checksum: vec![4, 5, 6],
                tier: 0,
            })
            .await;
        queue
            .push(Pcp {
                signup_id: SignupId::default(),
                user_id: "test".to_string(),
                data: vec![7, 8, 9],
                checksum: vec![10, 11, 12],
                tier: 0,
            })
            .await;
        assert_eq!(
            queue.pop().await,
            Some((
                Pcp {
                    signup_id: SignupId::default(),
                    user_id: "test".to_string(),
                    data: vec![1, 2, 3],
                    checksum: vec![4, 5, 6],
                    tier: 0,
                },
                0
            ))
        );
        assert_eq!(
            queue.pop().await,
            Some((
                Pcp {
                    signup_id: SignupId::default(),
                    user_id: "test".to_string(),
                    data: vec![7, 8, 9],
                    checksum: vec![10, 11, 12],
                    tier: 0,
                },
                1
            ))
        );
        assert_eq!(queue.pop().await, None);

        // Uncommited changes, simulating a crash.
        let mut queue = Queue::new(tempdir.path().to_path_buf()).await;
        assert_eq!(
            queue.pop().await,
            Some((
                Pcp {
                    signup_id: SignupId::default(),
                    user_id: "test".to_string(),
                    data: vec![1, 2, 3],
                    checksum: vec![4, 5, 6],
                    tier: 0,
                },
                0
            ))
        );
        assert_eq!(
            queue.pop().await,
            Some((
                Pcp {
                    signup_id: SignupId::default(),
                    user_id: "test".to_string(),
                    data: vec![7, 8, 9],
                    checksum: vec![10, 11, 12],
                    tier: 0,
                },
                1
            ))
        );
        assert_eq!(queue.pop().await, None);
        queue.commit(0).await;
        queue.commit(1).await;
        queue
            .push(Pcp {
                signup_id: SignupId::default(),
                user_id: "test".to_string(),
                data: vec![13, 14, 15],
                checksum: vec![16, 17, 18],
                tier: 0,
            })
            .await;
        assert_eq!(
            queue.pop().await,
            Some((
                Pcp {
                    signup_id: SignupId::default(),
                    user_id: "test".to_string(),
                    data: vec![13, 14, 15],
                    checksum: vec![16, 17, 18],
                    tier: 0,
                },
                0
            ))
        );
        queue.commit(0).await;

        // The directory should be empty now.
        let mut queue = Queue::new(tempdir.path().to_path_buf()).await;
        assert_eq!(queue.pop().await, None);
    }
}
