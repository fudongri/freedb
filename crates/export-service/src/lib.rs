use anyhow::Result;
use core_domain::QueryResult;
use csv::QuoteStyle;
use rust_xlsxwriter::Workbook;
use std::path::Path;

pub mod sql_dump;

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

    pub fn export_query_result_xlsx(&self, result: &QueryResult, path: impl AsRef<Path>) -> Result<()> {
        let mut workbook = Workbook::new();
        let worksheet = workbook.add_worksheet();
        // 写表头
        for (col, name) in result.columns.iter().enumerate() {
            worksheet.write_string(0, col as u16, name)?;
        }
        // 写数据
        for (row_idx, row) in result.rows.iter().enumerate() {
            let r = (row_idx + 1) as u32;
            for (col, col_name) in result.columns.iter().enumerate() {
                match row.get(col_name) {
                    Some(val) => {
                        let text = val.display_text();
                        // 尝试解析数字
                        if val.is_null() {
                            worksheet.write_string(r, col as u16, "")?;
                        } else if let Ok(n) = text.parse::<f64>() {
                            worksheet.write_number(r, col as u16, n)?;
                        } else {
                            worksheet.write_string(r, col as u16, text)?;
                        }
                    }
                    None => { worksheet.write_string(r, col as u16, "")?; }
                }
            }
        }
        workbook.save(path)?;
        Ok(())
    }

    pub fn export_query_result_sql(&self, result: &QueryResult, table_name: &str, path: impl AsRef<Path>) -> Result<()> {
        use std::io::Write;
        let mut file = std::fs::File::create(path)?;
        writeln!(file, "-- Exported table: {table_name}")?;
        writeln!(file, "-- Rows: {}", result.rows.len())?;
        writeln!(file)?;
        let col_list = result.columns.iter().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(", ");
        for row in &result.rows {
            let values = result.columns.iter().map(|c| {
                match row.get(c) {
                    Some(val) if !val.is_null() => {
                        let text = val.display_text();
                        // 数字直接输出，字符串转义
                        if text.parse::<f64>().is_ok() {
                            text.to_string()
                        } else {
                            format!("'{}'", text.replace('\'', "''"))
                        }
                    }
                    _ => "NULL".to_string(),
                }
            }).collect::<Vec<_>>().join(", ");
            writeln!(file, "INSERT INTO `{table_name}` ({col_list}) VALUES ({values});")?;
        }
        Ok(())
    }
}
