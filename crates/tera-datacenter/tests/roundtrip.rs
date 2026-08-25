use tera_datacenter::build::{BuildValue, Builder};
use tera_datacenter::{query, DataCenter, Value};

fn sample_builder() -> Builder {
    let mut builder = Builder::new();
    let sheet_name = builder.name_index("ItemData");
    let item_name = builder.name_index("Item");
    let id = builder.name_index("id");
    let label = builder.name_index("name");
    let weight = builder.name_index("weight");
    let tradable = builder.name_index("tradable");
    let sheet = builder.add_node(sheet_name);
    let root = builder.root;
    builder.attach(root, sheet);
    for index in 1..=64i32 {
        let item = builder.add_node(item_name);
        builder.attach(sheet, item);
        let text = builder.value_index(&format!("item {index}"));
        let node = builder.node_mut(item);
        node.attributes.push((id, BuildValue::Int(index)));
        node.attributes.push((label, BuildValue::Str(text)));
        node.attributes
            .push((weight, BuildValue::Float(index as f32 / 4.0)));
        node.attributes
            .push((tradable, BuildValue::Bool(index % 2 == 0)));
    }
    builder
}

#[test]
fn pack_then_parse() {
    let builder = sample_builder();
    let image = builder.pack().unwrap();
    let dc = DataCenter::from_inflated(image).unwrap();
    assert_eq!(dc.header.version, 6);
    let root = dc.root().unwrap();
    assert_eq!(root.name().unwrap(), "__root__");
    assert_eq!(root.child_count(), 1);
    let sheet = root.children().next().unwrap();
    assert_eq!(sheet.name().unwrap(), "ItemData");
    assert_eq!(sheet.child_count(), 64);

    let found = query(dc.root().unwrap(), "/ItemData/Item[@id=\"7\"]").unwrap();
    assert_eq!(found.len(), 1);
    let item = &found[0];
    assert_eq!(item.get("name").unwrap(), Value::Str("item 7".into()));
    assert_eq!(item.get("weight").unwrap(), Value::Float(1.75));
    assert_eq!(item.get("tradable").unwrap(), Value::Bool(false));
}

#[test]
fn identical_subtrees_are_shared() {
    let mut builder = Builder::new();
    let sheet_name = builder.name_index("Sheet");
    let leaf_name = builder.name_index("Leaf");
    let attribute = builder.name_index("value");
    let sheet = builder.add_node(sheet_name);
    let root = builder.root;
    builder.attach(root, sheet);
    for _ in 0..100 {
        let parent = builder.add_node(leaf_name);
        builder.attach(sheet, parent);
        let child = builder.add_node(leaf_name);
        builder.attach(parent, child);
        builder
            .node_mut(child)
            .attributes
            .push((attribute, BuildValue::Int(42)));
    }
    let image = builder.pack().unwrap();
    let dc = DataCenter::from_inflated(image).unwrap();
    assert_eq!(dc.node_count(), 103);
    assert_eq!(dc.attribute_count(), 1);
}

#[test]
fn round_trip_through_xml() {
    let builder = sample_builder();
    let image = builder.pack().unwrap();
    let dc = DataCenter::from_inflated(image).unwrap();
    let mut xml = Vec::new();
    let sheet = dc.root().unwrap().children().next().unwrap();
    tera_datacenter::export::write_xml(&mut xml, &sheet, true).unwrap();
    let text = String::from_utf8(xml).unwrap();

    let template = tera_datacenter::Template::from_datacenter(&dc).unwrap();
    let mut importer = tera_datacenter::Importer::new(Some(&template));
    importer.read_str(&text).unwrap();
    let rebuilt = DataCenter::from_inflated(importer.builder.pack().unwrap()).unwrap();

    let mut again = Vec::new();
    let sheet = rebuilt.root().unwrap().children().next().unwrap();
    tera_datacenter::export::write_xml(&mut again, &sheet, true).unwrap();
    assert_eq!(String::from_utf8(again).unwrap(), text);
}

#[test]
fn hashes_match_reference() {
    use tera_datacenter::hash::{string_hash, value_hash};
    assert_eq!(string_hash(""), 0);
    assert_eq!(value_hash(""), 0);
    assert_eq!(string_hash("__root__"), string_hash("__root__"));
    assert_ne!(string_hash("a"), string_hash("b"));
    assert_eq!(value_hash("Item"), value_hash("ITEM"));
    assert_eq!(value_hash("item"), value_hash("ITEM"));
}
