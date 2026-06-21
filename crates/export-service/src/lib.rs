use anyhow::Result;
use core_domain::QueryResult;
use csv::QuoteStyle;
use std::path::Path;

#[derive(Clone, Default)]
pub struct ExportService;

impl ExportService {
    pub fn export_query_result_csv(&self, result: &QueryResult, path: impl AsRef<Path>) -> Result<()> {
        let mut writer = csv::WriterBuilder::new()
            .quote_style(QuoteStyle::Always)
            .from_path(path)?;
        writer.write_record(&result.columns)?;
        for row in &result.rows {
            let record = result
                .columns
                .iter()
                .map(|column| {
                    row.get(column)
                        .map(|value| value.display_text().to_string())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>();
            writer.write_record(record)?;
        }
        writer.flush()?;
        Ok(())
    }
}
