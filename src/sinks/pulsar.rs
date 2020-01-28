use crate::{
    buffers::Acker,
    event::{self, Event},
    sinks::util::MetadataFuture,
    topology::config::{DataType, SinkConfig, SinkContext, SinkDescription},
};
use futures::{
    future::{self, poll_fn, IntoFuture},
    stream::FuturesUnordered,
    Async, AsyncSink, Future, Poll, Sink, StartSend, Stream,
};
use pulsar::{Consumer, Producer, PulsarExecutor, Pulsar};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use std::{collections::HashSet, path::PathBuf, time::Duration};
use string_cache::DefaultAtom as Atom;
use tokio::runtime::Runtime;

#[derive(Debug, Snafu)]
enum BuildError {
    #[snafu(display("creating pulsar producer failed: {}", source))]
    PulsarSinkFailed { source: pulsar::Error },
    #[snafu(display("invalid path: {:?}", path))]
    InvalidPath { path: PathBuf },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PulsarSinkConfig {
    address: String,
    topic: String,
    // key_field: Option<Atom>,
    encoding: Encoding,
    // tls: Option<KafkaSinkTlsConfig>,
}

#[derive(Deserialize, Serialize, Debug, Eq, PartialEq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    Text,
    Json,
}

pub struct PulsarSink {
    // producer: FutureProducer,
    topic: String,
    key_field: Option<Atom>,
    encoding: Encoding,
    // in_flight: FuturesUnordered<MetadataFuture<DeliveryFuture, usize>>,
    acker: Acker,
    seq_head: usize,
    seq_tail: usize,
    pending_acks: HashSet<usize>,
}

inventory::submit! {
    SinkDescription::new_without_default::<PulsarSinkConfig>("pulsar")
}

#[typetag::serde(name = "pulsar")]
impl SinkConfig for PulsarSinkConfig {
    fn build(&self, cx: SinkContext) -> crate::Result<(super::RouterSink, super::Healthcheck)> {
        let sink = PulsarSink::new(self.clone(), cx.acker());
        // healthcheck
        Ok((sink, hc))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn sink_type(&self) -> &'static str {
        "pulsar"
    }
}

impl PulsarSink {
    fn create(config: PulsarSinkConfig, acker: Acker) -> crate::Result<Box<dyn Future<Item = (), Error = ()>>> {
        let mut rt = Runtime::new()?;

        Box::new(future::lazy(move || {

            let pulsar = Pulsar::new(config.address,None,rt.executor()).and_then(| )
        }))
    }
}