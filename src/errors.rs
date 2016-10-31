//! Errors used in cratesfyi

use std::io;
use rustc_serialize::json;
use postgres;
use cargo;
use hyper;
use magic::MagicError;


error_chain! {
    types {
        Error, ErrorKind, ChainErr, Result;
    }

    links {
    }

    foreign_links {
        io::Error, IoError;
        json::BuilderError, JsonBuilderError;
        postgres::error::ConnectError, PostgresConnectError;
        postgres::error::Error, PostgresError;
        hyper::Error, HyperError;
        MagicError, MagicError;
        Box<cargo::CargoError>, CargoError;
    }

    errors {
    }
}
