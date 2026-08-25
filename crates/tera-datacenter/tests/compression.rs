use tera_datacenter::build::{BuildValue, Builder};
use tera_datacenter::{deflate, inflate};

fn repetitive_image(items: i32) -> Vec<u8> {
    let mut builder = Builder::new();
    let sheet = builder.name_index("ItemData");
    let item = builder.name_index("Item");
    let id = builder.name_index("id");
    let label = builder.name_index("name");
    let root = builder.root;
    let group = builder.add_node(sheet);
    builder.attach(root, group);
    for number in 1..=items {
        let node = builder.add_node(item);
        builder.attach(group, node);
        let text = builder.value_index("the same string every time");
        let node = builder.node_mut(node);
        node.attributes.push((id, BuildValue::Int(number)));
        node.attributes.push((label, BuildValue::Str(text)));
    }
    builder.pack().expect("pack")
}

#[test]
fn a_highly_compressible_image_still_produces_a_complete_stream() {
    let image = repetitive_image(64);
    assert!(image.len() > 1_000_000, "fixture should be large");
    let packed = deflate(&image, 6).expect("deflate");
    assert!(
        packed.len() > 4,
        "deflate returned only the length prefix, the stream was never finished"
    );
    assert_eq!(inflate(&packed).expect("inflate"), image);
}

#[test]
fn images_of_many_sizes_round_trip() {
    for items in [1i32, 8, 64, 512] {
        let image = repetitive_image(items);
        let packed = deflate(&image, 6).expect("deflate");
        assert_eq!(
            inflate(&packed).expect("inflate"),
            image,
            "{items} items did not survive deflate then inflate"
        );
    }
}
