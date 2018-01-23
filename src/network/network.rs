use std::collections::BTreeMap;
use std::fmt;
use std::mem;
use std::iter::{Iterator, Sum};
use random::{random, shuffle};
use network::prefix::Prefix;
use network::node::Node;
use network::section::Section;
use network::churn::{NetworkEvent, SectionEvent};
use params::Params;
use stats::Stats;

/// A wrapper struct that handles merges in progress
/// When two sections merge, they need to handle a bunch
/// of churn events before they actually become a single
/// section. This remembers which sections are in the
/// process of merging and reports whether all of them are
/// ready to be combined.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PendingMerge {
    complete: BTreeMap<Prefix, bool>,
}

impl PendingMerge {
    /// Creates a new "pending merge" from a set of prefixes - the prefixes passed
    /// are the ones that are supposed to merge
    fn from_prefixes<I: IntoIterator<Item = Prefix>>(pfxs: I) -> Self {
        PendingMerge {
            complete: pfxs.into_iter().map(|pfx| (pfx, false)).collect(),
        }
    }

    /// Mark a prefix as having completed the merge
    fn completed(&mut self, pfx: Prefix) {
        if let Some(entry) = self.complete.get_mut(&pfx) {
            *entry = true;
        }
    }

    /// Returns whether the sections are ready to be combined into one
    fn is_done(&self) -> bool {
        self.complete.iter().all(|(_, &complete)| complete)
    }

    /// Throws out the wrapper layer and returns the pure map
    fn into_map(self) -> BTreeMap<Prefix, bool> {
        self.complete
    }
}

#[derive(Clone, Default)]
pub struct NetworkStructure {
    pub size: usize,
    pub sections: usize,
    pub complete: usize,
}

#[derive(Clone, Default)]
pub struct Output {
    /// the number of "add" random events
    pub adds: u64,
    /// the number of "drop" random events
    pub drops: u64,
    /// the distribution of drops by age
    pub drops_dist: BTreeMap<u8, usize>,
    /// the number of "rejoin" random events
    pub rejoins: u64,
    /// the number of relocations
    pub relocations: u64,
    /// the number of rejected nodes
    pub rejections: u64,
    /// the total number of churn events
    pub churn: u64,
    /// the structure of the network
    pub network_structure: Vec<NetworkStructure>,
}

/// The structure representing the whole network
/// It's a container for sections that simulates all the
/// churn and communication between them.
#[derive(Clone)]
pub struct Network {
    /// all the sections in the network indexed by prefixes
    nodes: BTreeMap<Prefix, Section>,
    /// the nodes that left the network and could rejoin in the future
    left_nodes: Vec<Node>,
    /// queues of events to be processed by each section
    event_queue: BTreeMap<Prefix, Vec<NetworkEvent>>,
    /// prefixes that are in the process of merging
    pending_merges: BTreeMap<Prefix, PendingMerge>,
    /// Simulation parameters
    params: Params,
    /// Simulation outputs
    output: Output,
}

impl Network {
    /// Starts a new network
    pub fn new(params: Params) -> Network {
        let mut nodes = BTreeMap::new();
        nodes.insert(Prefix::empty(), Section::new(Prefix::empty()));
        Network {
            nodes,
            left_nodes: Vec::new(),
            event_queue: BTreeMap::new(),
            pending_merges: BTreeMap::new(),
            params,
            output: Default::default(),
        }
    }

    /// Checks whether there are any events in the queues
    fn has_events(&self) -> bool {
        self.event_queue.values().any(|x| !x.is_empty())
    }

    fn capture_network_structure(&mut self) {
        let structure = NetworkStructure {
            size: self.nodes.values().map(|x| x.len()).sum(),
            sections: self.nodes.len(),
            complete: self.nodes.values().filter(|x| x.is_complete()).count(),
        };
        self.output.network_structure.push(structure);
    }

    /// Sends all events to the corresponding sections and processes the events passed
    /// back. The responses generate new events and the cycle continues until the queues are empty.
    /// Then. if any pending merges are ready, they are processed, too.
    pub fn process_events(&mut self) {
        while self.has_events() {
            let queue = mem::replace(&mut self.event_queue, BTreeMap::new());
            for (prefix, events) in queue {
                let mut section_events = vec![];
                for event in events {
                    let params = &self.params;
                    let result = self.nodes
                        .get_mut(&prefix)
                        .map(|section| section.handle_event(event, params))
                        .unwrap_or_else(Vec::new);
                    section_events.extend(result);
                    if let NetworkEvent::PrefixChange(pfx) = event {
                        if let Some(pending_merge) = self.pending_merges.get_mut(&pfx) {
                            pending_merge.completed(prefix);
                        }
                    }
                }
                for section_event in section_events {
                    self.process_single_event(prefix, section_event);
                }
            }
        }
        let merges_to_finalise: Vec<_> = self.pending_merges
            .iter()
            .filter(|&(_, pm)| pm.is_done())
            .map(|(pfx, _)| *pfx)
            .collect();
        for pfx in merges_to_finalise {
            info!("Finalising a merge into {:?}", pfx);
            self.output.churn += 1; // counting merge as a single churn event
            let pending_merge = self.pending_merges.remove(&pfx).unwrap().into_map();
            let merged_section = self.merged_section(pending_merge.keys(), true);
            self.nodes.insert(merged_section.prefix(), merged_section);
        }
        self.capture_network_structure();
    }

    /// Processes a single response from a section and potentially inserts some events into its
    /// queue
    fn process_single_event(&mut self, prefix: Prefix, event: SectionEvent) {
        match event {
            SectionEvent::NodeDropped(node) => {
                self.left_nodes.push(node);
            }
            SectionEvent::NeedRelocate(node) => {
                self.relocate(node);
            }
            SectionEvent::NodeRejected(_) => {
                self.output.rejections += 1;
            }
            SectionEvent::RequestMerge => {
                self.merge(prefix);
            }
            SectionEvent::RequestSplit => {
                if let Some(section) = self.nodes.remove(&prefix) {
                    let ((sec0, ev0), (sec1, ev1)) = section.split(&self.params);
                    let _ = self.event_queue.remove(&prefix);
                    self.event_queue
                        .entry(sec0.prefix())
                        .or_insert_with(Vec::new)
                        .extend(ev0);
                    self.event_queue
                        .entry(sec1.prefix())
                        .or_insert_with(Vec::new)
                        .extend(ev1);
                    self.nodes.insert(sec0.prefix(), sec0);
                    self.nodes.insert(sec1.prefix(), sec1);
                    self.output.churn += 1; // counting the split as one churn event
                }
            }
        }
    }

    /// Returns the section that would be the result of merging sections with the given prefixes.
    /// If `destructive` is true, the sections are actually removed from `self.nodes` to be
    /// combined.
    fn merged_section<'a, I: IntoIterator<Item = &'a Prefix> + Clone>(
        &mut self,
        prefixes: I,
        destructive: bool,
    ) -> Section {
        let mut sections: Vec<_> = prefixes
            .clone()
            .into_iter()
            .filter_map(|pfx| {
                if destructive {
                    let _ = self.event_queue.remove(pfx);
                    self.nodes.remove(pfx)
                } else {
                    self.nodes.get(pfx).cloned()
                }
            })
            .collect();

        while sections.len() > 1 {
            sections.sort_by_key(|s| s.prefix());
            let section1 = sections.pop().unwrap();
            let section2 = sections.pop().unwrap();
            let section = section1.merge(section2, &self.params);
            sections.push(section);
        }

        sections.pop().unwrap()
    }

    /// Calculates which sections will merge into a given prefix, creates a pending merge for them
    /// and prepares queues for churn events to be processed before the merge itself.
    fn merge(&mut self, prefix: Prefix) {
        let merged_pfx = prefix.shorten();
        if let Some(&compatible_merge) = self.pending_merges
            .keys()
            .find(|pfx| pfx.is_compatible_with(&merged_pfx))
        {
            if compatible_merge.is_ancestor(&merged_pfx) {
                return;
            }
            let _ = self.pending_merges.remove(&compatible_merge);
        }
        info!("Initiating a merge into {:?}", merged_pfx);
        let prefixes: Vec<_> = self.nodes
            .keys()
            .filter(|&pfx| merged_pfx.is_ancestor(pfx))
            .cloned()
            .collect();

        let pending_merge = PendingMerge::from_prefixes(prefixes.iter().cloned());
        self.pending_merges.insert(merged_pfx, pending_merge);

        let merged_section = self.merged_section(prefixes.iter(), false);
        for pfx in prefixes {
            let events = self.calculate_merge_events(&merged_section, pfx);
            let _ = self.event_queue.insert(pfx, events);
        }
    }

    /// Creates the queue of events to be processed by a section `pfx` when it merges into
    /// `merged`.
    fn calculate_merge_events(&self, merged: &Section, pfx: Prefix) -> Vec<NetworkEvent> {
        let old_elders = self.nodes.get(&pfx).unwrap().elders();
        let new_elders = merged.elders();
        let mut events = vec![NetworkEvent::StartMerge(merged.prefix())];
        for lost_elder in &old_elders - &new_elders {
            events.push(NetworkEvent::Gone(lost_elder));
        }
        for gained_elder in &new_elders - &old_elders {
            events.push(NetworkEvent::Live(gained_elder, false));
        }
        events.push(NetworkEvent::PrefixChange(merged.prefix()));
        events
    }

    /// Adds a random node to the network by pushing an appropriate event to the queue
    pub fn add_random_node(&mut self) {
        self.output.adds += 1;
        self.output.churn += 1;
        let node = Node::new(random(), self.params.init_age);
        info!("Adding node {:?}", node);
        let prefix = self.prefix_for_node(node).unwrap();
        self.event_queue
            .entry(prefix)
            .or_insert_with(Vec::new)
            .push(NetworkEvent::Live(node, true));
    }

    /// Calculates the sum of weights for the dropping probability.
    /// When choosing the node to be dropped, every node is assigned a weight, so that older nodes
    /// have less chance of dropping. This helps in calculating which node should be dropped.
    // fn total_drop_weight(&self) -> f64 {
    //     self.nodes
    //         .iter()
    //         .flat_map(|(_, s)| s.nodes().into_iter())
    //         .map(|n| n.drop_probability(self.params.drop_dist))
    //         .sum()
    // }

    /// Returns the prefix a node should belong to.
    fn prefix_for_node(&self, node: Node) -> Option<Prefix> {
        self.nodes
            .keys()
            .find(|pfx| pfx.matches(node.name()))
            .cloned()
    }

    /// Chooses a new section for the given node, generates a new name for it,
    /// increases its age,  and sends a `Live` event to the section.
    fn relocate(&mut self, mut node: Node) {
        self.output.relocations += 1;
        self.output.churn += 2; // leaving one section and joining another one
        let (node, neighbour) = {
            let src_section = self.nodes
                .keys()
                .find(|&pfx| pfx.matches(node.name()))
                .unwrap();
            let mut neighbours: Vec<_> = self.nodes
                .keys()
                .filter(|&pfx| pfx.is_neighbour(src_section))
                .collect();
            // relocate to the neighbour with the least peers as per the document
            neighbours.sort_by_key(|pfx| pfx.len() as usize * 10000 + self.nodes.get(pfx).unwrap().len());
            let neighbour = if let Some(n) = neighbours.first() {
                n
            } else {
                src_section
            };
            let old_node = node.clone();
            node.relocate(neighbour);
            info!(
                "Relocating {:?} from {:?} to {:?} as {:?}",
                old_node, src_section, neighbour, node
            );
            (node, neighbour)
        };
        self.event_queue
            .entry(*neighbour)
            .or_insert_with(Vec::new)
            .push(NetworkEvent::Live(node, true));
    }

    /// Drops a random node from the network by sending a `Lost` event to the section.
    /// The probability of a given node dropping is weighted based on its age.
    pub fn drop_random_node(&mut self) {
        let node_and_prefix = {
            let mut res = None;
            let nodes_iter = self.nodes
                .iter()
                .flat_map(|(p, s)| s.sort_by_age().into_iter().map(move |n| (*p, n)));
            for (p, n) in nodes_iter {
                let drop = random::<usize>();
                if drop % 2.0f64.powf(n.age() as f64) as usize == 0 {
                    res = Some((p, n));
                    break;
                }
            }
            res
        };
        node_and_prefix.map(|(prefix, node)| {
            self.output.drops += 1;
            self.output.churn += 1;
            *self.output.drops_dist.entry(node.age()).or_insert(0) += 1;
            let name = node.name();
            info!("Dropping node {:?} from section {:?}", name, prefix);
            self.event_queue
                .entry(prefix)
                .or_insert_with(Vec::new)
                .push(NetworkEvent::Lost(name));
        });
    }

    /// Chooses a random node from among the ones that left the network and gets it to rejoin.
    /// The age of the rejoining node is reduced.
    pub fn rejoin_random_node(&mut self) {
        self.output.rejoins += 1;
        self.output.churn += 1;
        shuffle(&mut self.left_nodes);
        if let Some(mut node) = self.left_nodes.pop() {
            info!("Rejoining node {:?}", node);
            node.rejoined(self.params.init_age);
            let prefix = self.prefix_for_node(node).unwrap();
            self.event_queue
                .entry(prefix)
                .or_insert_with(Vec::new)
                .push(NetworkEvent::Live(node, true));
        }
    }

    pub fn num_sections(&self) -> usize {
        self.nodes.len()
    }

    pub fn age_distribution(&self) -> BTreeMap<u8, usize> {
        let mut result = BTreeMap::new();
        for (_, section) in &self.nodes {
            for node in section.nodes() {
                *result.entry(node.age()).or_insert(0) += 1;
            }
        }
        result
    }

    pub fn complete_sections(&self) -> usize {
        self.nodes.iter().filter(|&(_, s)| s.is_complete()).count()
    }

    pub fn output(&self) -> &Output {
        &self.output
    }
}

impl fmt::Debug for Network {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(
            fmt,
            "Network {{\n\tadds: {}\n\tdrops: {}\n\trejoins: {}\n\trelocations: {}\n\trejections: {}\n\ttotal churn: {}\n\ttotal nodes: {}\n\n{:?}\nleft_nodes: {:?}\n\n}}",
            self.output.adds,
            self.output.drops,
            self.output.rejoins,
            self.output.relocations,
            self.output.rejections,
            self.output.churn,
            usize::sum(self.nodes.values().map(|s| s.len())),
            self.nodes.values(),
            self.left_nodes
        )
    }
}

// Display network summary as a markdown table
impl fmt::Display for Network {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        // Network summary
        try!(writeln!(fmt, "|    Metrics    |  Values  |"));
        try!(writeln!(fmt, "|:--------------|---------:|"));
        try!(writeln!(fmt, "| Adds          | {:>8} |", self.output.adds));
        try!(writeln!(fmt, "| Drops         | {:>8} |", self.output.drops));
        try!(writeln!(fmt, "| Rejoins       | {:>8} |", self.output.rejoins));
        try!(writeln!(fmt, "| Relocations   | {:>8} |", self.output.relocations));
        try!(writeln!(fmt, "| Rejections    | {:>8} |", self.output.rejections));
        try!(writeln!(fmt, "| Churns        | {:>8} |", self.output.churn));
        let sections = self.num_sections();
        try!(writeln!(fmt, "| Sections      | {:>8} |", sections));
        let complete = self.complete_sections();
        if complete != sections {
            try!(writeln!(fmt, "| Complete      | {:>8} |", complete));
        }
        try!(writeln!(fmt, "| Section nodes | {:>8} |", usize::sum(self.nodes.values().map(|s| s.len()))));
        try!(writeln!(fmt, "| Left nodes    | {:>8} |", self.left_nodes.len()));
        try!(writeln!(fmt));

        // Distribution of sections per prefix length
        let mut distribution : BTreeMap<u8, Vec<usize>> = BTreeMap::new();
        for (pfx, section) in &self.nodes {
            let mut entry = distribution.entry(pfx.len()).or_insert(Vec::new());
            entry.push(section.len());
        }
        let mut lengths: Vec<u8> = distribution.keys().cloned().collect();
        lengths.sort();
        try!(writeln!(fmt, "| Prefix len {}", Stats::get_header_line()));
        try!(writeln!(fmt, "|:-----------{}", Stats::get_separator_line()));
        for i in lengths {
            try!(writeln!(fmt, "| {:>10} | {}", i, Stats::new(distribution.get(&i).unwrap())))
        }
        writeln!(fmt, "|        All | {}", Stats::new(&self.nodes.values().map(|s| s.len()).collect()))
    }
}
