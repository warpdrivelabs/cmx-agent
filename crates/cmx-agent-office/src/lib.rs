//! cmx-agent 办公文档（U6）：读 Excel/PDF/Word/PPT + 写 Excel。重依赖隔离在本 crate。
pub mod excel;
pub mod text;
pub mod tool;

pub use tool::{DocReadTool, XlsxWriteTool};
