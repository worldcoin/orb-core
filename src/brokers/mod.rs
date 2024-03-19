//! Collection of brokers.

mod observer;
mod orb;

use crate::{agents::Agent, port};

pub use self::{
    observer::{
        Builder as ObserverBuilder, DefaultPlan as DefaultObserverPlan, Observer,
        Plan as ObserverPlan,
    },
    orb::{Builder, Orb, Plan as OrbPlan, StateRx as OrbStateRx},
};

use futures::prelude::*;
use std::{mem::replace, pin::Pin};

/// Future to kill an agent.
pub type AgentKill = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Agent cell inside a broker.
pub enum AgentCell<T: Agent> {
    /// Agent is not initialized.
    Vacant,
    /// Agent is initialized and enabled.
    Enabled((port::Outer<T>, AgentKill)),
    /// Agent is initialized but disabled.
    Disabled((port::Outer<T>, AgentKill)),
}

/// Used to tell a broker whether it should exit early or go on as usual.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BrokerFlow {
    /// Continue managing agents.
    Continue,
    /// Stops the broker returning control to the caller.
    Break,
}

impl<T: Agent> AgentCell<T> {
    /// Returns `Some(port)` if the agent is enabled, otherwise returns `None`.
    pub fn enabled(&mut self) -> Option<&mut port::Outer<T>> {
        match self {
            Self::Vacant | Self::Disabled(_) => None,
            Self::Enabled((ref mut port, _kill)) => Some(port),
        }
    }

    /// Returns `true` if the agent is enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled(_))
    }

    /// Returns `true` if the agent is initialized.
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        !matches!(self, Self::Vacant)
    }

    /// Kills the agent.
    pub async fn kill(&mut self) {
        match replace(self, Self::Vacant) {
            Self::Enabled((_port, kill)) | Self::Disabled((_port, kill)) => kill.await,
            Self::Vacant => {}
        }
    }
}
