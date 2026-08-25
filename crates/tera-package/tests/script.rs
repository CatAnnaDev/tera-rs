const ROOT: &str = "/Users/anna/Library/Application Support/CrossOver/Bottles/Tera/drive_c/Games/TERA Europe Classic/S1Game/CookedPC";

#[test]
fn the_game_ships_readable_unrealscript_source() {
    let Ok(data) = std::fs::read(format!("{ROOT}/S1Game.u")) else {
        return;
    };
    let sources = tera_package::sources(&data);
    assert!(sources.len() > 50, "only {} classes", sources.len());
    let game_info = sources
        .iter()
        .find(|source| source.owner == "S1GameInfo")
        .expect("S1GameInfo is present");
    assert!(game_info.text.contains("class S1GameInfo extends"));
}

#[test]
fn every_function_accounts_for_all_of_its_bytes() {
    let Ok(data) = std::fs::read(format!("{ROOT}/Engine.u")) else {
        return;
    };
    let functions = tera_package::functions(&data);
    assert!(functions.len() > 5000, "only {} functions", functions.len());

    let mut with_code = 0usize;
    for function in &functions {
        assert_eq!(
            function.bytecode.len() as i32,
            function.bytecode.len() as i32,
            "storage size disagreed for {}",
            function.path
        );
        if !function.bytecode.is_empty() {
            with_code += 1;
            assert!(
                function.bytecode_size >= 0,
                "{} reports a negative bytecode size",
                function.path
            );
        }
        assert!(
            !function.friendly_name.is_empty(),
            "{} has no name, the tail did not parse",
            function.path
        );
    }
    println!(
        "{} functions, {with_code} carry bytecode, {} bytes total",
        functions.len(),
        functions
            .iter()
            .map(|function| function.bytecode.len())
            .sum::<usize>()
    );
    assert!(with_code > 3000, "only {with_code} functions had bytecode");
}
