//! Errors used in cratesfyi

use std::io;
use rustc_serialize::json;
use postgres;
use cargo;
use reqwest;
use magic::MagicError;
use git2;
use regex;


error_chain! {
    foreign_links {
        IoError(io::Error);
        JsonBuilderError(json::BuilderError);
        PostgresConnectError(postgres::error::ConnectError);
        PostgresError(postgres::error::Error);
        ReqwestError(reqwest::Error);
        Git2Error(git2::Error);
        MagicError(MagicError);
        CargoError(Box<cargo::CargoError>);
        RegexError(regex::Error);
    }
}
