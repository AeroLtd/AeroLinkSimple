use crate::node_packets::{LoraParam, NodeWorkingMode};

pub mod node_packets;
pub mod node_codec;
pub mod ui;
pub mod serial_manager;


#[derive(Clone)]
pub struct NodeConfig {
    pub port_name: String,
    pub params: LoraParam,
    pub mode: NodeWorkingMode,
}