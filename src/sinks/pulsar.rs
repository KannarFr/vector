use crate::{
    buffers::Acker,
    event::{self, Event},
    runtime::TaskExecutor,
    sinks::util::MetadataFuture,
    topology::config::{DataType, SinkConfig, SinkContext, SinkDescription},
};
use futures::{
    future::{self, poll_fn, IntoFuture},
    stream::FuturesUnordered,
    Async, AsyncSink, Future, Poll, Sink, StartSend, Stream,
};
use pulsar::{
    proto::CommandSendReceipt, Consumer, Producer, ProducerOptions, Pulsar, PulsarExecutor,
    SerializeMessage,
};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use std::{collections::HashSet, path::PathBuf};

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
    encoding: Encoding,
    batch_size: Option<u32>,
}

#[derive(Deserialize, Serialize, Debug, Eq, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    Text,
    Json,
}

pub struct PulsarSink {
    topic: String,
    encoding: Encoding,
    producer: Producer,
    pulsar: Pulsar,
    in_flight: FuturesUnordered<SendFuture>,
    seq_head: usize,
    seq_tail: usize,
    seqno: HashSet<usize>,
}

pub type SendFuture = Box<dyn Future<Item = (CommandSendReceipt, usize), Error = pulsar::Error>>;
// pub type MetadataFuture<F, M> = future::Join<F, future::FutureResult<M, <F as Future>::Error>>;

inventory::submit! {
    SinkDescription::new_without_default::<PulsarSinkConfig>("pulsar")
}

#[typetag::serde(name = "pulsar")]
impl SinkConfig for PulsarSinkConfig {
    fn build(&self, cx: SinkContext) -> crate::Result<(super::RouterSink, super::Healthcheck)> {
        Ok((
            Box::new(PulsarSink::new(self.clone(), cx.exec())?),
            healthcheck(self.clone()),
        ))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn sink_type(&self) -> &'static str {
        "pulsar"
    }
}

impl PulsarSink
// where
// F: Future<Item = CommandSendReceipt, Error = pulsar::Error> + 'static + Send,
{
    fn new(config: PulsarSinkConfig, exec: TaskExecutor) -> crate::Result<Self> {
        let pulsar = Pulsar::new(config.address.parse()?, None, exec).wait()?;
        let producer = pulsar.producer(Some(ProducerOptions {
            batch_size: config.batch_size,
            ..ProducerOptions::default()
        }));

        Ok(Self {
            topic: config.topic,
            encoding: config.encoding,
            pulsar,
            producer,
            in_flight: FuturesUnordered::new(),
            seq_head: 0,
            seq_tail: 0,
            seqno: HashSet::new(),
        })
    }
}

impl Sink for PulsarSink
// where
// F: Future<Item = CommandSendReceipt, Error = pulsar::Error> + 'static + Send,
{
    type SinkItem = Event;
    type SinkError = ();

    fn start_send(&mut self, item: Self::SinkItem) -> StartSend<Self::SinkItem, Self::SinkError> {
        let message = encode_event(item, &self.topic, self.encoding).map_err(|_| ())?;
        let seqno = self.seq_head;
        self.seq_head += 1;
        let fut = future::lazy(move || {
            self.producer
                .send(self.topic.clone(), &PulsarSend(message))
                .join(future::ok(seqno))
        });
        self.in_flight.push(Box::new(fut));
        Ok(AsyncSink::Ready)
    }

    fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
        loop {
            match self.in_flight.poll() {
                Ok(Async::NotReady) => return Ok(Async::NotReady),
                Ok(Async::Ready(None)) => return Ok(Async::Ready(())),
                Ok(Async::Ready(Some(result))) => {
                    trace!(
                        "produced message {:?} from {} at sequence id {}",
                        result.message_id,
                        result.producer_id,
                        result.sequence_id
                    );
                }
                Err(e) => error!("future cancelled: {}", e),
            }
        }
    }
}

// TODO: https://github.com/wyyerd/pulsar-rs/issues/60
// #[derive(Clone)]
struct PulsarSend(Vec<u8>);

impl SerializeMessage for PulsarSend {
    fn serialize_message(input: &Self) -> Result<pulsar::producer::Message, pulsar::Error> {
        Ok(pulsar::producer::Message {
            payload: input.0.clone(),
            ..Default::default()
        })
    }
}

fn encode_event<S: AsRef<str>>(item: Event, topic: S, enc: Encoding) -> crate::Result<Vec<u8>> {
    let log = item.into_log();
    let data = match enc {
        Encoding::Json => serde_json::to_vec(&log.unflatten())?,
        Encoding::Text => log
            .get(&event::MESSAGE)
            .map(|v| v.as_bytes().to_vec())
            .unwrap_or_default(),
    };
    Ok(data)
}

fn healthcheck(config: PulsarSinkConfig) -> super::Healthcheck {
    unimplemented!()
}
