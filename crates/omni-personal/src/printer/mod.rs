//! The fixed Brother HL-L2370DW monochrome LAN printer (`src/printer/`).

pub mod ipp;
pub mod pipeline;
pub mod service;

pub use service::{
    AcceptedPrintJob, AcceptedPrintRecord, PrintPaper, PrintPdfInput, PrintSides,
    PrinterDependencies, PrinterError, PrinterService, PrinterStatus,
};
