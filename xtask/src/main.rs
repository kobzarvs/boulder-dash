mod caves;
mod chr;
mod music;
mod params;
mod rom;
mod screens;
mod sprites;

use anyhow::Result;
use std::path::PathBuf;

pub struct Ctx {
    pub data_dir: PathBuf,
    pub assets_dir: PathBuf,
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(cmd) = args.next() else {
        eprintln!("usage: xtask extract[-caves|-chr|-params|-music|-screens|-sprites] <rom.nes> [out-base=game]");
        std::process::exit(2);
    };
    let rom_path = PathBuf::from(args.next().expect("rom path required"));
    let base = args.next().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("game"));
    let ctx = Ctx {
        data_dir: base.join("src/data"),
        assets_dir: base.join("assets"),
    };
    std::fs::create_dir_all(&ctx.data_dir)?;
    std::fs::create_dir_all(&ctx.assets_dir)?;

    let rom = rom::Rom::load(&rom_path)?;
    match cmd.as_str() {
        "extract-caves" => caves::extract(&rom, &ctx)?,
        "extract-chr" => chr::extract(&rom, &ctx)?,
        "extract-params" => params::extract(&rom, &ctx)?,
        "extract-music" => music::extract(&rom, &ctx)?,
        "extract-screens" => screens::extract(&rom, &ctx)?,
        "extract-sprites" => sprites::extract(&rom, &ctx)?,
        "extract" => {
            caves::extract(&rom, &ctx)?;
            chr::extract(&rom, &ctx)?;
            params::extract(&rom, &ctx)?;
            music::extract(&rom, &ctx)?;
            screens::extract(&rom, &ctx)?;
            sprites::extract(&rom, &ctx)?;
        }
        other => anyhow::bail!("unknown command `{other}`"),
    }
    println!("done");
    Ok(())
}
