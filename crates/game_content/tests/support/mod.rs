#![allow(dead_code)]
use game_types::ContentId;
use std::{
    fs,
    path::{Path, PathBuf},
};
pub struct Temp(pub PathBuf);
impl Temp {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!("yarra-content-test-{}", ContentId::new()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    pub fn source(&self) -> PathBuf {
        let root = self.0.join("source");
        copy(&sample(), &root);
        root
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
pub fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/gameplay/demo")
}
fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target)
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
pub fn write(path: impl AsRef<Path>, value: &impl serde::Serialize) {
    fs::write(
        path,
        ron::ser::to_string_pretty(value, ron::ser::PrettyConfig::default()).unwrap(),
    )
    .unwrap();
}
pub fn read<T: serde::de::DeserializeOwned>(path: impl AsRef<Path>) -> T {
    ron::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}
/// Publish the project and start its scenario against the published bundle.
pub fn runtime(
    temp: &Temp,
    project: &yarra_game_content::LoadedProject,
) -> yarra_game_content::RuntimeSession {
    let bundle = temp.0.join("content.sqlite");
    project.build(&bundle).unwrap();
    gameplay::GameSession::new(
        yarra_game_content::ContentRepository::open(bundle).unwrap(),
        project.start().unwrap().into_state(),
    )
    .unwrap()
}
