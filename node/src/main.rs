use commonware_consensus::{
    simplex::{
        config,
        elector::RoundRobin,
        mocks::{application, relay, reporter},
        scheme::ed25519,
        Engine,
    },
    types::{Delta, Epoch, View},
    Monitor,
};
use commonware_cryptography::{
    ed25519::PublicKey as Ed25519PublicKey, sha256::Digest as Sha256Digest, Sha256,
};
use commonware_p2p::simulated::{
    Config as NetworkConfig, Link, Network, Receiver as P2pReceiver, Sender as P2pSender,
};
use commonware_parallel::Sequential;
use commonware_runtime::{buffer::paged::CacheRef, deterministic, Metrics, Quota, Runner, Spawner};
use commonware_utils::{channel::mpsc::Receiver, NZU16, NZUsize};
use futures::future::join_all;
use std::{collections::HashMap, num::NonZeroU32, sync::Arc, time::Duration};

const NAMESPACE: &[u8] = b"krypto_l1";
const NUM_VALIDATORS: u32 = 4;
const REQUIRED_BLOCKS: u64 = 10;

// Type aliases for readability
type Channel = (
    P2pSender<Ed25519PublicKey, deterministic::Context>,
    P2pReceiver<Ed25519PublicKey>,
);
type MyReporter =
    reporter::Reporter<deterministic::Context, ed25519::Scheme, RoundRobin, Sha256Digest>;

fn main() {
    let executor = deterministic::Runner::new(deterministic::Config::new());
    executor.start(|mut context| async move {
        // --- Network ---
        let (network, oracle) = Network::new(
            context.with_label("network"),
            NetworkConfig {
                max_size: 1024 * 1024,
                disconnect_on_block: false,
                tracked_peer_sets: None,
            },
        );
        network.start();

        // --- Validator keys ---
        let fixture = ed25519::fixture(&mut context, NAMESPACE, NUM_VALIDATORS);
        let participants = fixture.participants;
        let schemes = fixture.schemes;

        // --- Register 3 p2p channels per validator ---
        // channel 0 = vote (pending), 1 = certificate (recovered), 2 = resolver
        let quota = Quota::per_second(NonZeroU32::MAX);
        let mut registrations: HashMap<Ed25519PublicKey, (Channel, Channel, Channel)> =
            HashMap::new();
        for validator in participants.iter() {
            let control = oracle.control(validator.clone());
            let pending = control.register(0, quota).await.unwrap();
            let recovered = control.register(1, quota).await.unwrap();
            let resolver = control.register(2, quota).await.unwrap();
            registrations.insert(validator.clone(), (pending, recovered, resolver));
        }

        // --- Fully-connected links between all pairs ---
        let link = Link {
            latency: Duration::from_millis(10),
            jitter: Duration::from_millis(1),
            success_rate: 1.0,
        };
        for v1 in participants.iter() {
            for v2 in participants.iter() {
                if v1 == v2 {
                    continue;
                }
                oracle.add_link(v1.clone(), v2.clone(), link.clone()).await.unwrap();
            }
        }

        // --- Shared in-memory relay for block propagation between validators ---
        let shared_relay = Arc::new(relay::Relay::new());

        let mut reporters: Vec<(usize, MyReporter)> = Vec::new();

        for (i, validator) in participants.iter().enumerate() {
            let scheme = schemes[i].clone();
            let (pending, recovered, resolver) = registrations.remove(validator).unwrap();
            let ctx = context.with_label(&format!("validator_{i}"));

            let elector = RoundRobin::default();

            // Reporter: verifies and records all consensus activity; exposes finalization events
            let reporter_cfg = reporter::Config {
                participants: participants.as_slice().try_into().expect("unique keys"),
                scheme: scheme.clone(),
                elector: elector.clone(),
            };
            let rep: MyReporter =
                reporter::Reporter::new(ctx.with_label("reporter"), reporter_cfg);

            // Application: proposes and verifies blocks (mock — random payloads)
            let app_cfg = application::Config {
                hasher: Sha256::default(),
                relay: shared_relay.clone(),
                me: validator.clone(),
                propose_latency: (10.0, 5.0),
                verify_latency: (10.0, 5.0),
                certify_latency: (10.0, 5.0),
                should_certify: application::Certifier::Sometimes,
            };
            let (actor, app) =
                application::Application::new(ctx.with_label("application"), app_cfg);
            actor.start();

            // Engine: runs simplex BFT consensus
            let blocker = oracle.control(validator.clone());
            let engine_cfg = config::Config {
                blocker,
                scheme,
                elector,
                automaton: app.clone(),
                relay: app.clone(),
                reporter: rep.clone(),
                partition: format!("validator_{i}"),
                mailbox_size: 1024,
                epoch: Epoch::new(0),
                leader_timeout: Duration::from_secs(1),
                certification_timeout: Duration::from_secs(2),
                timeout_retry: Duration::from_secs(10),
                fetch_timeout: Duration::from_secs(1),
                activity_timeout: Delta::new(10),
                skip_timeout: Delta::new(5),
                fetch_concurrent: 1,
                replay_buffer: NZUsize!(1024 * 1024),
                write_buffer: NZUsize!(1024 * 1024),
                page_cache: CacheRef::from_pooler(&ctx, NZU16!(1024), NZUsize!(10)),
                strategy: Sequential,
            };
            let engine = Engine::new(ctx.with_label("engine"), engine_cfg);
            engine.start(pending, recovered, resolver);

            reporters.push((i, rep));
        }

        // --- Wait for all validators to finalize REQUIRED_BLOCKS views ---
        let mut finalizers = Vec::new();
        for (i, mut rep) in reporters {
            let required = REQUIRED_BLOCKS;
            let (mut latest, mut monitor): (View, Receiver<View>) = rep.subscribe().await;
            finalizers.push(
                context
                    .with_label(&format!("finalizer_{i}"))
                    .spawn(move |_| async move {
                        while latest.get() < required {
                            println!("[validator_{i}] finalized view {}", latest.get());
                            latest = monitor.recv().await.expect("monitor closed");
                        }
                        println!("[validator_{i}] ✓ reached {required} finalized blocks");
                    }),
            );
        }
        join_all(finalizers).await;

        println!("All validators finalized {REQUIRED_BLOCKS} blocks. Consensus works!");
    });
}
