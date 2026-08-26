# iOS

Open `Yarra/Yarra.xcodeproj`, select a physical iPhone, and run the `Yarra`
scheme. The Xcode build phase cross-compiles the Rust executable and packages
the cooked runtime database, shaders, character GLB, and imported tree and
terrain runtime files.

Use the Release build configuration for performance measurements. The Debug
configuration is intended only for iteration and validation.

The physical-device Rust target is installed with:

```sh
rustup target add aarch64-apple-ios
```

The simulator is optional and is not suitable for GPU performance testing. An
Apple Silicon simulator additionally requires:

```sh
rustup target add aarch64-apple-ios-sim
```
