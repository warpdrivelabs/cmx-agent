//! cmx-agent 办公文档（U6/U7）：读 Excel/PDF/Word/PPT + 写 Excel + 生成 PPT。重依赖隔离在本 crate。
pub mod excel;
pub mod pptx;
pub mod text;
pub mod tool;

pub use tool::{DocReadTool, PptxWriteTool, XlsxWriteTool};
