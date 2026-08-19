//! 数据同步 pipeline 的公共入口。
//!
//! 这里先提供一套轻量级抽象：每个同步任务由一组 step 串起来，
//! 后续新增直播同步、补充审核流程、扩展字段转换时，可以复用执行器和通用能力。

pub mod activity;
pub mod audit_notice;
pub mod audit_result_sync;
pub mod bitable;
pub mod live;
pub mod project_report;
pub mod runner;
pub mod sheet;
pub mod sync;
pub(crate) mod table_sync;
pub mod video;
pub mod workflow;
