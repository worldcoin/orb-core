use clap::{Parser, Subcommand};
use csv::WriterBuilder;
use eyre::{Context, Result};
use orb::debug_report::{DebugReport, DEBUG_REPORT_VERSION};
use schema_traversal::ControlSchemaMetadata;
use schemars::{gen::SchemaSettings, schema::RootSchema, JsonSchema};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{fs::File, process::ExitCode};
use thiserror::Error;

mod schema_traversal;

#[derive(Parser)]
struct Cli {
    #[clap(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand)]
enum CliCommand {
    /// Check if the DEBUG_REPORT_VERSION is correct
    CheckVersion,
    /// Export the DebugReport Schema in JSON and CSV formats
    Export,
}

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        CliCommand::CheckVersion => {
            match schema_to_hash::<DebugReport>(version_hash_schema_settings()) {
                Ok(current_schema_hash) => {
                    if DEBUG_REPORT_VERSION != current_schema_hash {
                        println!("Detected DebugReport updates.");
                        println!("Update DEBUG_REPORT_VERSION with: {}", current_schema_hash);
                        return Ok(ExitCode::FAILURE);
                    }

                    println!("DebugReport version is up to date.");
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) => match e.downcast_ref::<SchemaError>() {
                    Some(SchemaError::CannotInline(names)) => {
                        println!(
                            "Some structs on your recent changes have nested fields with structs \
                             of the same name:"
                        );
                        for name in names {
                            println!("- {}", name);
                        }
                        println!("This is not supported:");
                        println!("struct Config {{\n\tcfg_a: a::Config,\n\tcfg_b: b::Config \n}}");
                        println!("To resolve the issue use #[schemars(rename = \"...\"]).");
                        Ok(ExitCode::FAILURE)
                    }
                    _ => Err(e),
                },
            }
        }
        CliCommand::Export => {
            write_schema_to_disk::<DebugReport>("debug_report_schema")?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

#[derive(Error, Debug)]
enum SchemaError {
    #[error("{0}")]
    EyreError(#[from] eyre::Error),
    #[error("Cannot inline nested structs with same names.")]
    CannotInline(Vec<String>),
}

fn get_root_schema<T: JsonSchema>(settings: SchemaSettings) -> RootSchema {
    settings.into_generator().into_root_schema_for::<T>()
}

fn schema_to_string<T: JsonSchema>(settings: SchemaSettings) -> Result<String, SchemaError> {
    let schema = get_root_schema::<T>(settings);
    if !schema.definitions.is_empty() {
        let struct_names = schema.definitions.keys().cloned().collect();
        return Err(SchemaError::CannotInline(struct_names));
    }

    Ok(serde_json::to_string(&schema).wrap_err("Failed to produce schema")?)
}

fn schema_to_hash<T: JsonSchema>(settings: SchemaSettings) -> Result<String> {
    let hash = Sha256::digest(schema_to_string::<T>(settings)?);
    Ok(format!("{:x}", hash))
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Serialize, Debug)]
pub struct CSVRecord {
    path: String,
    instance_type: String,
}

fn write_schema_to_disk<T: JsonSchema>(file_basename: &str) -> Result<()> {
    let mut schema = get_root_schema::<T>(output_files_schema_settings());
    let records = schema_traversal::collect_csv_records(&mut schema);

    let csv_file =
        File::create(file_basename.to_owned() + ".csv").wrap_err("Failed to create CSV file.")?;
    let mut wtr = WriterBuilder::new().from_writer(csv_file);

    for record in records {
        wtr.serialize(record).wrap_err("Failed to serialize record.")?;
    }

    wtr.flush().wrap_err("Failed to write into CSV file.")?;
    std::fs::write(
        file_basename.to_owned() + ".json",
        schema_to_string::<T>(output_files_schema_settings())?,
    )
    .wrap_err("Failed to write into JSON file.")?;
    Ok(())
}

#[must_use]
fn version_hash_schema_settings() -> SchemaSettings {
    SchemaSettings::default()
        .with(|s| {
            s.inline_subschemas = true;
        })
        .with_visitor(ControlSchemaMetadata { exclude_description: true, ..Default::default() })
}

#[must_use]
fn output_files_schema_settings() -> SchemaSettings {
    SchemaSettings::default().with(|s| {
        s.inline_subschemas = true;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    mod a {
        use super::*;
        #[derive(JsonSchema)]
        pub struct Config {
            field: i32,
        }
    }

    #[allow(dead_code)]
    mod b {
        use super::*;
        #[derive(JsonSchema)]
        pub struct Config {
            field: f64,
        }
    }

    #[allow(dead_code)]
    #[derive(JsonSchema)]
    struct Config {
        cfg_a: a::Config,
        cfg_b: b::Config,
    }

    #[test]
    fn test_naming_conflict_detected() {
        // This test is added to ensure that we detect this [issue](https://github.com/GREsau/schemars/issues/62).
        // Delete this test if a library update fixes the above.
        let schema_string = schema_to_string::<Config>(version_hash_schema_settings());
        assert!(schema_string.is_err());
    }

    #[allow(dead_code)]
    #[derive(JsonSchema)]
    struct Coordinates {
        x: f32,
        y: f32,
    }

    #[allow(dead_code)]
    #[derive(JsonSchema)]
    struct Foo {
        bar: String,
    }

    #[allow(dead_code)]
    #[derive(JsonSchema)]
    enum PointRender {
        Dot,
        Square(Foo),
        Triangle,
    }

    #[allow(dead_code)]
    #[derive(JsonSchema)]
    struct Point {
        coords: Coordinates,
        index: u32,
        point_render: PointRender,
    }

    #[test]
    fn test_csv_record_collection() {
        let expected_output = vec![
            CSVRecord { path: "coords".to_owned(), instance_type: "Object".to_owned() },
            CSVRecord { path: "coords/x".to_owned(), instance_type: "Number".to_owned() },
            CSVRecord { path: "coords/y".to_owned(), instance_type: "Number".to_owned() },
            CSVRecord { path: "index".to_owned(), instance_type: "Integer".to_owned() },
            CSVRecord { path: "point_render".to_owned(), instance_type: "".to_owned() },
            CSVRecord {
                path: "point_render/Dot".to_owned(),
                instance_type: "Enum Variant".to_owned(),
            },
            CSVRecord {
                path: "point_render/Square".to_owned(),
                instance_type: "Object".to_owned(),
            },
            CSVRecord {
                path: "point_render/Square/bar".to_owned(),
                instance_type: "String".to_owned(),
            },
            CSVRecord {
                path: "point_render/Triangle".to_owned(),
                instance_type: "Enum Variant".to_owned(),
            },
        ];

        let mut root = get_root_schema::<Point>(output_files_schema_settings());
        let actual_output = schema_traversal::collect_csv_records(&mut root);

        assert_eq!(actual_output, expected_output);
    }
}
