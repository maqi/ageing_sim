use network::prefix::{Name, Prefix};
use network::node::{Digest, Node};
use serde_json;
use tiny_keccak::sha3_256;

/// Events that can happen in the network.
/// The sections handle them and generate new ones
/// in the process. Some events can also be generated from
/// the outside.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum NetworkEvent {
    // Boolean parameter indicates if event should count for node ageing.
    // It is true except for the specific case of a Live event generated during a merge operation
    Live(Node, bool),
    Lost(Name),
    Gone(Node),
    Relocated(Node),
    PrefixChange(Prefix),
    StartMerge(Prefix),
}

impl NetworkEvent {
    /// Returns the digest of some representation of the network event:
    /// used in ageing (to determine if a peer should be relocated).
    pub fn hash(&self) -> Digest {
        sha3_256(&serde_json::to_vec(self).unwrap())
    }

    /// This function determines whether an event should count towards
    /// churn in ageing peers in the section. Currently true for all events.
    pub fn should_count(&self) -> bool {
        match *self {
            NetworkEvent::StartMerge(_) | NetworkEvent::Gone(_) | NetworkEvent::Live(_, false) => false,
            _ => true,
        }
    }
}

/// Events reported by the sections to the network.
/// The network processes them and responds with churn
/// events that the nodes would add to their data chains
/// in the real network.
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SectionEvent {
    NodeDropped(Node),
    NodeRejected(Node),
    NeedRelocate(Node),
    RequestMerge,
    RequestSplit,
}
