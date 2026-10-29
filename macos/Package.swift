// swift-tools-version:6.0
// ReelStudio: the Mac app for reelctl (docs/specs/mac-app.md). Built with
// `swift build`; `make -C macos app dmg` packages it with the reelctl binary.

import PackageDescription

let package = Package(
    name: "ReelStudio",
    platforms: [.macOS(.v15)],
    products: [
        .library(name: "ReelCore", targets: ["ReelCore"]),
        .executable(name: "ReelStudio", targets: ["ReelStudio"]),
    ],
    targets: [
        .target(
            name: "ReelCore",
            path: "Sources/ReelCore"
        ),
        .executableTarget(
            name: "ReelStudio",
            dependencies: ["ReelCore"],
            path: "Sources/ReelStudio"
        ),
        .testTarget(
            name: "ReelCoreTests",
            dependencies: ["ReelCore"],
            path: "Tests/ReelCoreTests"
        ),
    ],
    swiftLanguageModes: [.v5]
)
