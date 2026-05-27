//! Multi-agent research system with planner, researcher, and writer agents.

mod planner;
mod researcher;
mod writer;

pub use planner::plan_research_node;
pub use researcher::research_sub_task;
pub use writer::write_report;

// Rust guideline compliant 2026-05-27
