//! Runs the real `crowbotd` binary against a scripted ACP agent
//! (`examples/fake_acp_agent.rs`) and talks to it over its real endpoint.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod bots;
mod claude;
mod conversation;
mod endpoint;
mod providers;
mod queue;
mod recovery;
mod runtimes;
mod settings;
mod shell;
mod startup;
mod stop;
mod support;
