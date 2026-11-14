//! Runs the real `crowbotd` binary against a scripted ACP agent
//! (`examples/fake_acp_agent.rs`) and talks to it over its real endpoint.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod bots;
mod claude;
mod endpoint;
mod queue;
mod recovery;
mod shell;
mod stop;
mod support;
