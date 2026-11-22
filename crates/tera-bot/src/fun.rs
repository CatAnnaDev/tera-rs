fn rand_u64() -> u64 {
    let mut bytes = [0u8; 8];
    let _ = getrandom::fill(&mut bytes);
    u64::from_le_bytes(bytes)
}

fn below(bound: u64) -> u64 {
    if bound == 0 {
        0
    } else {
        rand_u64() % bound
    }
}

fn pick<'a>(options: &[&'a str]) -> &'a str {
    options[below(options.len() as u64) as usize]
}

pub fn dice(argument: &str) -> String {
    let spec = argument.trim();
    let (count, faces) = match spec.split_once(['d', 'D']) {
        Some((c, f)) => (
            c.trim().parse::<u32>().unwrap_or(1).clamp(1, 50),
            f.trim().parse::<u32>().unwrap_or(6).clamp(2, 1000),
        ),
        None if spec.is_empty() => (1, 6),
        None => (1, spec.parse::<u32>().unwrap_or(6).clamp(2, 1000)),
    };
    let mut total = 0u64;
    let mut rolls = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let value = below(faces as u64) + 1;
        total += value;
        rolls.push(value.to_string());
    }
    if count == 1 {
        format!("de {faces} -> {total}")
    } else {
        format!("{count}d{faces} -> {total} ({})", rolls.join("+"))
    }
}

pub fn flip() -> String {
    format!("pile ou face -> {}", pick(&["PILE", "FACE"]))
}

pub fn roll() -> String {
    format!("roll 1-100 -> {}", below(100) + 1)
}

pub fn eightball(question: &str) -> String {
    const ANSWERS: &[&str] = &[
        "c'est certain",
        "sans aucun doute",
        "oui, clairement",
        "tu peux compter dessus",
        "probablement",
        "les signes disent oui",
        "difficile a dire, reessaie",
        "demande plus tard",
        "mieux vaut ne pas te le dire maintenant",
        "ma reponse est non",
        "mes sources disent non",
        "tres improbable",
        "n'y compte pas",
    ];
    let answer = pick(ANSWERS);
    if question.trim().is_empty() {
        format!("8ball -> {answer}")
    } else {
        format!("8ball ({}) -> {answer}", question.trim())
    }
}

pub fn rps(argument: &str) -> String {
    let moves = ["pierre", "feuille", "ciseaux"];
    let bot = pick(&moves);
    let player = argument.trim().to_lowercase();
    let player = match player.as_str() {
        "pierre" | "caillou" | "p" | "rock" => Some("pierre"),
        "feuille" | "papier" | "f" | "paper" => Some("feuille"),
        "ciseaux" | "c" | "scissors" => Some("ciseaux"),
        _ => None,
    };
    let Some(player) = player else {
        return format!("pfc : joue 'pierre', 'feuille' ou 'ciseaux' (moi: {bot})");
    };
    let outcome = match (player, bot) {
        (a, b) if a == b => "egalite",
        ("pierre", "ciseaux") | ("feuille", "pierre") | ("ciseaux", "feuille") => "tu gagnes",
        _ => "je gagne",
    };
    format!("pfc : toi {player} vs moi {bot} -> {outcome}")
}

pub fn choose(argument: &str) -> String {
    let options: Vec<&str> = argument
        .split(['|', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if options.len() < 2 {
        return "choisis : donne au moins 2 options separees par | ou ,".to_string();
    }
    format!("je choisis -> {}", pick(&options))
}

pub fn slot() -> String {
    const REEL: &[&str] = &["7", "BAR", "cerise", "cloche", "etoile", "trefle"];
    let a = pick(REEL);
    let b = pick(REEL);
    let c = pick(REEL);
    let line = format!("[ {a} | {b} | {c} ]");
    if a == b && b == c {
        format!("{line} JACKPOT !!!")
    } else if a == b || b == c || a == c {
        format!("{line} presque !")
    } else {
        format!("{line} perdu")
    }
}

pub fn fortune() -> String {
    const FORTUNES: &[&str] = &[
        "un drop rare t'attend aujourd'hui",
        "mefie-toi des BAM dans le dos",
        "ta prochaine enchant passera, promis",
        "quelqu'un pense a t'inviter en groupe",
        "le broker te sourit ce soir",
        "range ton inventaire avant qu'il deborde",
        "un vieux ami se reconnecte bientot",
        "aujourd'hui, esquive tout",
    ];
    format!("fortune -> {}", pick(FORTUNES))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_de_reste_dans_les_bornes() {
        for _ in 0..200 {
            let out = dice("1d6");
            let value: u64 = out.rsplit("-> ").next().unwrap().parse().unwrap();
            assert!((1..=6).contains(&value), "{out}");
        }
    }

    #[test]
    fn choisis_exige_deux_options() {
        assert!(choose("seul").contains("au moins 2"));
        assert!(choose("a | b").starts_with("je choisis"));
    }

    #[test]
    fn pile_ou_face_est_binaire() {
        for _ in 0..50 {
            let out = flip();
            assert!(out.ends_with("PILE") || out.ends_with("FACE"), "{out}");
        }
    }
}

pub fn help() -> String {
    "fun: de [NdM] | pile | roll | 8ball <question> | pfc <coup> | choisis a|b|c | slot | fortune".to_string()
}
