//! Source-specific living and player presentation.
mod modern;
mod motion;
#[cfg(test)]
pub(crate) use motion::BodyRule;
pub(crate) use motion::MotionFacts;
mod state;
pub(crate) use state::{Modern, State, TickInput, folds_wings};
