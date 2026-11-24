//! Runs the real `crowbotd` binary against a scripted ACP agent
//! (`examples/fake_acp_agent.rs`) and talks to it over its real endpoint.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod approvals;
mod bots;
mod claude;
mod conversation;
mod endpoint;
#[cfg(target_os = "linux")]
mod guard;
mod prompt;
mod providers;
mod queue;
mod recovery;
mod runtimes;
#[cfg(target_os = "linux")]
mod service;
mod settings;
mod shell;
#[cfg(unix)]
mod signals;
mod startup;
mod stop;
mod support;
mod tools;
mod turns;
