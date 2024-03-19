use super::CSVRecord;
use schemars::{
    schema::{RootSchema, Schema, SchemaObject, SingleOrVec},
    visit::Visitor,
};
use serde_json::Value;
struct SchemaPathExplorer {
    current_path: Vec<String>,
    csv_records: Vec<CSVRecord>,
}

pub fn collect_csv_records(root: &mut RootSchema) -> Vec<CSVRecord> {
    let mut visitor = SchemaPathExplorer { current_path: Vec::new(), csv_records: Vec::new() };
    visitor.visit_root_schema(root);
    visitor.csv_records
}

impl Visitor for SchemaPathExplorer {
    fn visit_schema_object(&mut self, schema: &mut SchemaObject) {
        if let Some(obj) = &mut schema.object {
            // override the default visitor
            for (name, s) in &mut obj.properties {
                self.current_path.push(name.clone());
                self.csv_records.push(CSVRecord {
                    path: self.current_path.join("/"),
                    instance_type: determine_type(s),
                });
                schemars::visit::visit_schema(self, s);
                self.current_path.pop();
            }
        } else {
            // fallback on the default visitor
            schemars::visit::visit_schema_object(self, schema);
        }

        // include enum variants
        if let Some(enum_values) = &schema.enum_values {
            for value in enum_values {
                if let Value::String(name) = value {
                    self.csv_records.push(CSVRecord {
                        path: self.current_path.join("/") + "/" + name,
                        instance_type: String::from("Enum Variant"),
                    });
                }
            }
        }
    }

    fn visit_root_schema(&mut self, root: &mut RootSchema) {
        schemars::visit::visit_root_schema(self, root);
        self.csv_records.sort();
    }
}

fn determine_type(schema: &Schema) -> String {
    if let Schema::Object(obj) = schema {
        if let Some(single_or_vec) = &obj.instance_type {
            match single_or_vec {
                SingleOrVec::Single(single) => {
                    return format!("{:?}", single);
                }
                SingleOrVec::Vec(vec) => {
                    return format!("{:?}", vec);
                }
            }
        }
    }
    String::from("")
}

#[derive(Default, Debug, Clone)]
pub struct ControlSchemaMetadata {
    pub exclude_id: bool,
    pub exclude_title: bool,
    pub exclude_description: bool,
    pub exclude_default: bool,
    pub exclude_examples: bool,
}

impl Visitor for ControlSchemaMetadata {
    fn visit_schema_object(&mut self, schema: &mut SchemaObject) {
        schemars::visit::visit_schema_object(self, schema);
        if let Some(metadata) = &mut schema.metadata {
            if self.exclude_id {
                metadata.id = None;
            }
            if self.exclude_title {
                metadata.title = None;
            }
            if self.exclude_description {
                metadata.description = None;
            }
            if self.exclude_default {
                metadata.default = None;
            }
            if self.exclude_examples {
                metadata.examples.clear();
            }
        }
    }
}
