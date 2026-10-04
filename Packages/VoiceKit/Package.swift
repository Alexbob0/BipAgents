// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "VoiceKit",
    platforms: [.iOS(.v18), .macOS(.v15)],
    products: [
        .library(name: "VoiceKit", targets: ["VoiceKit"])
    ],
    targets: [
        .target(name: "VoiceKit"),
        .testTarget(name: "VoiceKitTests", dependencies: ["VoiceKit"])
    ],
    swiftLanguageModes: [.v6]
)
