fn main() {
    const NOTICES: &str = "resources/THIRD_PARTY_NOTICES.txt";
    println!("cargo:rerun-if-changed={NOTICES}");
    if std::env::var("PROFILE").is_ok_and(|profile| profile == "release") {
        let notices = std::fs::read_to_string(NOTICES).expect("read dependency notices");
        assert!(
            notices.contains("Generated from locked, target-filtered dependency metadata"),
            "Generate release notices first: python scripts/dependency-notices.py --artifact studio \
             --out studio/apps/viva-studio-tauri/src-tauri/resources/THIRD_PARTY_NOTICES.txt (from repo root)"
        );
    }
    tauri_build::build()
}
