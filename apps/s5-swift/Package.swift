// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "ShardrailSwift",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "ShardrailCore", targets: ["ShardrailCore"]),
        .executable(name: "shardrail-swift", targets: ["ShardrailApp"]),
    ],
    targets: [
        .target(name: "ShardrailCore", linkerSettings: [.linkedFramework("CryptoKit")]),
        .executableTarget(name: "ShardrailApp", dependencies: ["ShardrailCore"], linkerSettings: [.linkedFramework("AppKit"), .linkedFramework("CoreVideo")]),
        .testTarget(name: "ShardrailCoreTests", dependencies: ["ShardrailCore"]),
    ]
)
