//! Music theory: the part of the composer that would be true without a computer.
//!
//! Nothing here draws a random number or knows what a clip is. Every type is a small value that
//! can be printed in a test failure and checked by hand against a chord chart.

pub mod chart;
pub mod chord;
pub mod key;
pub mod numeral;
pub mod pitch;
pub mod scale;
