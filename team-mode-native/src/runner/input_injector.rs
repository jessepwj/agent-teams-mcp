use std::io::{self, Write};

use super::protocol::InjectStrategyWire;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectionStrategy {
    PasteAndEnter,
    BracketedPaste,
    CtrlC,
}

impl From<InjectStrategyWire> for InjectionStrategy {
    fn from(value: InjectStrategyWire) -> Self {
        match value {
            InjectStrategyWire::PasteAndEnter => Self::PasteAndEnter,
            InjectStrategyWire::BracketedPaste => Self::BracketedPaste,
            InjectStrategyWire::CtrlC => Self::CtrlC,
        }
    }
}

impl From<InjectionStrategy> for InjectStrategyWire {
    fn from(value: InjectionStrategy) -> Self {
        match value {
            InjectionStrategy::PasteAndEnter => Self::PasteAndEnter,
            InjectionStrategy::BracketedPaste => Self::BracketedPaste,
            InjectionStrategy::CtrlC => Self::CtrlC,
        }
    }
}

pub fn format_injected_input(text: &str, strategy: InjectionStrategy) -> Vec<u8> {
    match strategy {
        InjectionStrategy::PasteAndEnter => {
            let mut payload = text.as_bytes().to_vec();
            if !text.ends_with('\n') && !text.ends_with("\r\n") {
                payload.push(b'\n');
            }
            payload
        }
        InjectionStrategy::BracketedPaste => {
            let mut payload = b"\x1b[200~".to_vec();
            payload.extend_from_slice(text.as_bytes());
            payload.extend_from_slice(b"\x1b[201~\n");
            payload
        }
        InjectionStrategy::CtrlC => vec![0x03],
    }
}

pub fn inject_into_writer(
    writer: &mut dyn Write,
    text: &str,
    strategy: InjectionStrategy,
) -> io::Result<()> {
    let payload = format_injected_input(text, strategy);
    writer.write_all(&payload)?;
    writer.flush()
}
