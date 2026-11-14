//! Private, unqualified exact geometry for the later balance sizing slice.
//! No production caller, public API, input certification, sizing or send grant.
mod arithmetic;
mod bridge;
mod geometry;
#[cfg(test)]
mod geometry_tests;
mod inputs;
#[cfg(test)]
mod test_support;
