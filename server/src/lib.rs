#![feature(proc_macro_hygiene, decl_macro)]
#![feature(core_intrinsics)]
#![feature(btree_drain_filter)]
#![feature(generators)]
#![allow(dead_code)]
#![deny(unused_must_use)]
pub mod rest_controller;
pub mod rtp_server;

pub(crate) const SERVER_REQUEST_VERSION: u64 = 1;
