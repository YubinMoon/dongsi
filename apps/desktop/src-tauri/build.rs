fn main() {
    // exe에 넣는 아이콘(resource)은 빌드 스크립트가 만든다. tauri-build는 icons/ 변경을 감시하지 않아서
    // 아이콘을 바꿔도 옛 아이콘이 남았다. 아이콘이 바뀌면 다시 돌게 한다.
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
