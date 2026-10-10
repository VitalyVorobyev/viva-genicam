mod error;
mod full_parser;
mod model;

pub use error::ParseError;
pub use full_parser::parse_genicam_xml;
pub use model::{
    EnumEntry, NumericConstraints, UiCategory, UiGraph, UiNode, UiNodeKind, UiSkippedNode,
};
