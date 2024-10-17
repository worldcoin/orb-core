use std::{
    convert::identity,
    fs,
    io::{self, prelude::*},
};
use toml::{Table, Value};

fn main() {
    let root = fs::read_to_string("Cargo.toml").unwrap();
    let root = toml::from_str::<Value>(&root).unwrap();
    let Some(ws) = root.as_table().unwrap().get("workspace") else {
        io::copy(&mut io::stdin(), &mut io::stdout()).unwrap();
        return;
    };

    let mut toml = String::new();
    io::stdin().read_to_string(&mut toml).unwrap();
    let mut toml = toml::from_str::<Value>(&toml).unwrap();

    if let Some(ws_deps) = ws.as_table().unwrap().get("dependencies").and_then(Value::as_table) {
        if let Some(deps) = toml.as_table_mut().unwrap().get_mut("dependencies") {
            merge_tables(deps.as_table_mut().unwrap(), ws_deps);
        }
        if let Some(deps) = toml.as_table_mut().unwrap().get_mut("dev-dependencies") {
            merge_tables(deps.as_table_mut().unwrap(), ws_deps);
        }
        if let Some(deps) = toml.as_table_mut().unwrap().get_mut("build-dependencies") {
            merge_tables(deps.as_table_mut().unwrap(), ws_deps);
        }
    }

    if let (Some(ws_pkg), Some(pkg)) = (
        ws.as_table().unwrap().get("package").and_then(Value::as_table),
        toml.as_table_mut().unwrap().get_mut("package").and_then(Value::as_table_mut),
    ) {
        merge_tables(pkg, ws_pkg);
    }

    println!("{}", toml::to_string_pretty(&toml).unwrap());
}

fn merge_tables(table: &mut Table, ws_table: &Table) {
    for (key, value) in table.iter_mut() {
        if !value
            .as_table()
            .and_then(|item| item.get("workspace"))
            .and_then(Value::as_bool)
            .is_some_and(identity)
        {
            continue;
        }
        let Some(ws_value) = ws_table.get(key) else {
            eprintln!("Couldn't find workspace value for key '{key}'");
            continue;
        };
        *value = ws_value.clone();
    }
}
