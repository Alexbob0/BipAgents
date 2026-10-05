// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "VoiceKit",
    defaultLocalization: "fr",
    platforms: [.iOS(.v18), .macOS(.v15)],
    products: [
        .library(name: "VoiceKit", targets: ["VoiceKit"])
    ],
    targets: [
        .target(name: "VoiceKit", resources: [.process("Resources")]),
        .testTarget(name: "VoiceKitTests", dependencies: ["VoiceKit"])
    ],
    swiftLanguageModes: [.v6]
)
