use anyhow::{Context, Result};
use futures::prelude::*;
use libp2p::{
    core::{muxing::StreamMuxerBox, upgrade},
    floodsub::{self, Floodsub, FloodsubEvent},
    identity,
    mdns::{Mdns, MdnsEvent},
    mplex,
    swarm::{ExpandedSwarm, NetworkBehaviour, NetworkBehaviourEventProcess, SwarmBuilder},
    tcp::TokioTcpConfig,
    NetworkBehaviour, PeerId, Swarm, Transport,
};
use libp2p_secio::SecioConfig;
use log::{debug, info};
use tokio::io::{self, AsyncBufReadExt};

type Libp2pTransport = libp2p::core::transport::Boxed<(PeerId, StreamMuxerBox)>;

// We create a custom network behaviour that combines floodsub and mDNS.
// The derive generates a delegating `NetworkBehaviour` impl which in turn
// requires the implementations of `NetworkBehaviourEventProcess` for
// the events of each behaviour.
#[derive(NetworkBehaviour)]
struct MyBehaviour {
    #[behaviour(ignore)]
    topic:    floodsub::Topic,
    floodsub: Floodsub,
    mdns:     Mdns,
}

impl NetworkBehaviourEventProcess<FloodsubEvent> for MyBehaviour {
    // Called when `floodsub` produces an event.
    fn inject_event(&mut self, message: FloodsubEvent) {
        if let FloodsubEvent::Message(message) = message {
            println!(
                "Received: '{:?}' from {:?}",
                String::from_utf8_lossy(&message.data),
                message.source
            );
        }
    }
}

impl NetworkBehaviourEventProcess<MdnsEvent> for MyBehaviour {
    // Called when `mdns` produces an event.
    fn inject_event(&mut self, event: MdnsEvent) {
        match event {
            MdnsEvent::Discovered(list) => {
                for (peer, _) in list {
                    self.floodsub.add_node_to_partial_view(peer);
                }
            }
            MdnsEvent::Expired(list) => {
                for (peer, _) in list {
                    if !self.mdns.has_node(&peer) {
                        self.floodsub.remove_node_from_partial_view(&peer);
                    }
                }
            }
        }
    }
}

impl MyBehaviour {
    async fn new(peer_id: PeerId) -> Result<Self> {
        // Create a Floodsub topic
        let floodsub_topic = floodsub::Topic::new("chat");

        let mdns = Mdns::new()
            .await
            .context("Creating mDNS node discovery behaviour")?;

        let mut behaviour = MyBehaviour {
            topic: floodsub_topic.clone(),
            floodsub: Floodsub::new(peer_id),
            mdns,
        };

        behaviour.floodsub.subscribe(floodsub_topic.clone());

        Ok(behaviour)
    }

    fn publish(&mut self, msg: &str) {
        self.floodsub.publish(self.topic.clone(), msg.as_bytes());
    }
}

pub async fn make_transport(peer_id_keys: identity::Keypair) -> Result<Libp2pTransport> {
    Ok(TokioTcpConfig::new()
        .nodelay(true)
        .upgrade(upgrade::Version::V1)
        .authenticate(SecioConfig::new(peer_id_keys.clone()))
        .multiplex(mplex::MplexConfig::new())
        .boxed())
}

pub async fn run() -> Result<()> {
    // Generate peer id
    let peer_id_keys = identity::Keypair::generate_ed25519();
    let peer_id = PeerId::from(peer_id_keys.public());
    info!("Peer Id: {}", peer_id.clone());

    // Create a transport
    let transport = make_transport(peer_id_keys.clone())
        .await
        .context("Creating libp2p transport")?;

    // Create node behaviour
    let behaviour = MyBehaviour::new(peer_id.clone())
        .await
        .context("Creating node behaviour")?;

    // Executor for connection background tasks.
    let executor = Box::new(|future| {
        debug!("Spawning background task");
        tokio::spawn(future);
    });

    // Create a Swarm to manage peers and events.
    let mut swarm: Swarm<MyBehaviour> = SwarmBuilder::new(transport, behaviour, peer_id)
        .executor(executor)
        .build();

    // Listen on all interfaces and whatever port the OS assigns
    Swarm::listen_on(
        &mut swarm,
        "/ip4/0.0.0.0/tcp/0"
            .parse()
            .context("Parsing listening address")?,
    )
    .context("Starting to listen")?;

    // Read full lines from stdin
    let mut stdin = io::BufReader::new(io::stdin()).lines();

    // Catch SIGTERM so the container can shutdown without an init process.
    let sigterm = tokio::signal::ctrl_c();
    tokio::pin!(sigterm);

    // Kick it off
    loop {
        tokio::select! {
            line = stdin.try_next() => {
                info!("Stdin: {:?}", &line);
                let msg = line?.expect("Stdin closed");
                swarm.publish(&msg);
            },
            event = swarm.next() => {
                info!("New Event: {:?}", event);
            },
            _ = &mut sigterm => {
                info!("SIGTERM received, shutting down");
                // TODO: Shut down swarm?
                break;
            }
        }
    }
    info!("Done.");
    // TODO: Somehow it blocks here waiting for stdin.

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use float_eq::assert_float_eq;
    use futures::stream::{self, StreamExt, TryStreamExt};
    use hyper::{
        body::{to_bytes, HttpBody},
        Request,
    };
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;
}

#[cfg(feature = "bench")]
pub(crate) mod bench {
    use super::*;
    use criterion::{black_box, Criterion};
    use futures::executor::block_on;
    use hyper::body::to_bytes;

    pub(crate) fn group(c: &mut Criterion) {}
}
