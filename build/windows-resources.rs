fn main() {
    println!("cargo:rerun-if-changed=../../assets/portal.ico");
    println!("cargo:rerun-if-env-changed=WINDRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref()!=Ok("windows") {return;}
    let root=std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let icon=root.join("assets/portal.ico").canonicalize().unwrap();
    let out=std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let input=out.join("portal.rc");
    let icon=icon.to_string_lossy().trim_start_matches(r"\\?\").replace('\\',"/");
    std::fs::write(&input,format!("1 ICON DISCARDABLE \"{icon}\"\n")).unwrap();
    let gnu=std::env::var("CARGO_CFG_TARGET_ENV").as_deref()==Ok("gnu");
    let output=out.join(if gnu {"portal-icon.o"}else{"portal-icon.res"});
    let status=if gnu {
        std::process::Command::new(std::env::var("WINDRES").unwrap_or_else(|_|"windres".into())).arg("--input").arg(&input).arg("--output").arg(&output).args(["--output-format","coff"]).status()
    }else{
        std::process::Command::new("rc").arg("/nologo").arg("/fo").arg(&output).arg(&input).status()
    }.expect("Windows resource compiler is required to embed the application icon");
    assert!(status.success(),"Failed to compile application icon");
    println!("cargo:rustc-link-arg-bins={}",output.display());
}
