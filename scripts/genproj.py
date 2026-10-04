# Generates BipAgents.xcodeproj/project.pbxproj (app + notification service extension).
APP_INFO = {
    "CFBundleDisplayName": "BipAgents",
    "NSCameraUsageDescription": "Pour prendre une photo ou scanner le QR code d’un agent.",
    "NSMicrophoneUsageDescription": "Pour parler à tes agents.",
    "NSPhotoLibraryUsageDescription": "Pour envoyer des photos à tes agents.",
    "NSSpeechRecognitionUsageDescription": "Pour transcrire ta voix directement sur l’iPhone.",
    "UIApplicationSceneManifest_Generation": "YES",
    "UIApplicationSupportsIndirectInputEvents": "YES",
    "UILaunchScreen_Generation": "YES",
    "UISupportedInterfaceOrientations_iPad": '"UIInterfaceOrientationPortrait UIInterfaceOrientationPortraitUpsideDown UIInterfaceOrientationLandscapeLeft UIInterfaceOrientationLandscapeRight"',
    "UISupportedInterfaceOrientations_iPhone": "UIInterfaceOrientationPortrait",
}

def q(v):
    v = str(v)
    if v.startswith('"') or (v.replace(".", "").replace("_", "").isalnum() and not v[0].isdigit() and " " not in v):
        return v
    return '"' + v.replace('"', '\\"') + '"'

def settings(d, indent="\t\t\t\t"):
    out = []
    for k in sorted(d):
        v = d[k]
        if isinstance(v, list):
            out.append(f"{indent}{k} = (\n" + "".join(f"{indent}\t{q(x)},\n" for x in v) + f"{indent});")
        else:
            out.append(f"{indent}{k} = {q(v)};")
    return "\n".join(out)

common_project = dict(ALWAYS_SEARCH_USER_PATHS="NO", ASSETCATALOG_COMPILER_GENERATE_SWIFT_ASSET_SYMBOL_EXTENSIONS="YES",
    CLANG_ENABLE_MODULES="YES", CLANG_ENABLE_OBJC_ARC="YES", COPY_PHASE_STRIP="NO", ENABLE_STRICT_OBJC_MSGSEND="YES",
    ENABLE_USER_SCRIPT_SANDBOXING="YES", IPHONEOS_DEPLOYMENT_TARGET="18.0", LOCALIZATION_PREFERS_STRING_CATALOGS="YES",
    SDKROOT="iphoneos", SWIFT_VERSION="6.0", DEVELOPMENT_TEAM='""')
proj_debug = dict(common_project, DEBUG_INFORMATION_FORMAT="dwarf", ENABLE_TESTABILITY="YES", GCC_OPTIMIZATION_LEVEL="0",
    MTL_ENABLE_DEBUG_INFO="INCLUDE_SOURCE", ONLY_ACTIVE_ARCH="YES", SWIFT_ACTIVE_COMPILATION_CONDITIONS='"DEBUG $(inherited)"',
    SWIFT_OPTIMIZATION_LEVEL='"-Onone"')
proj_release = dict(common_project, DEBUG_INFORMATION_FORMAT='"dwarf-with-dsym"', ENABLE_NS_ASSERTIONS="NO",
    SWIFT_COMPILATION_MODE="wholemodule", VALIDATE_PRODUCT="YES")

app = dict(ASSETCATALOG_COMPILER_APPICON_NAME="AppIcon", ASSETCATALOG_COMPILER_GLOBAL_ACCENT_COLOR_NAME="AccentColor",
    CODE_SIGN_ENTITLEMENTS="Config/BipAgents.entitlements", CODE_SIGN_STYLE="Automatic", CURRENT_PROJECT_VERSION="1",
    ENABLE_PREVIEWS="YES", GENERATE_INFOPLIST_FILE="YES", INFOPLIST_FILE="Config/Info.plist",
    LD_RUNPATH_SEARCH_PATHS=["$(inherited)", "@executable_path/Frameworks"], MARKETING_VERSION="0.1",
    PRODUCT_BUNDLE_IDENTIFIER="io.github.bipagents", PRODUCT_NAME='"$(TARGET_NAME)"', SWIFT_APPROACHABLE_CONCURRENCY="YES",
    SWIFT_DEFAULT_ACTOR_ISOLATION="MainActor", SWIFT_EMIT_LOC_STRINGS="YES", SWIFT_UPCOMING_FEATURE_MEMBER_IMPORT_VISIBILITY="YES",
    TARGETED_DEVICE_FAMILY='"1,2"')
for k, v in APP_INFO.items():
    app["INFOPLIST_KEY_" + k] = v if v.startswith('"') else f'"{v}"' if " " in v else v
nse = dict(CODE_SIGN_ENTITLEMENTS="Config/NotificationService.entitlements", CODE_SIGN_STYLE="Automatic", CURRENT_PROJECT_VERSION="1",
    GENERATE_INFOPLIST_FILE="YES", INFOPLIST_FILE='"Config/NotificationService-Info.plist"', INFOPLIST_KEY_CFBundleDisplayName="NotificationService",
    LD_RUNPATH_SEARCH_PATHS=["$(inherited)", "@executable_path/Frameworks", "@executable_path/../../Frameworks"], MARKETING_VERSION="0.1",
    PRODUCT_BUNDLE_IDENTIFIER="io.github.bipagents.NotificationService", PRODUCT_NAME='"$(TARGET_NAME)"', SKIP_INSTALL="YES",
    SWIFT_APPROACHABLE_CONCURRENCY="YES", SWIFT_EMIT_LOC_STRINGS="YES", SWIFT_UPCOMING_FEATURE_MEMBER_IMPORT_VISIBILITY="YES",
    TARGETED_DEVICE_FAMILY='"1,2"')

def cfg(id_, name, d):
    return f"\t\t{id_} /* {name} */ = {{\n\t\t\tisa = XCBuildConfiguration;\n\t\t\tbuildSettings = {{\n{settings(d)}\n\t\t\t}};\n\t\t\tname = {name};\n\t\t}};\n"

P = "AA00000000000000000"  # id prefix (24 hex chars total)
def i(n): return f"{P}{n:05X}"

objs = f"""/* Begin PBXBuildFile section */
		{i(0x101)} /* HermesKit in Frameworks */ = {{isa = PBXBuildFile; productRef = {i(0x201)} /* HermesKit */; }};
		{i(0x102)} /* VoiceKit in Frameworks */ = {{isa = PBXBuildFile; productRef = {i(0x202)} /* VoiceKit */; }};
		{i(0x103)} /* NotificationService.appex in Embed Foundation Extensions */ = {{isa = PBXBuildFile; fileRef = {i(0x303)} /* NotificationService.appex */; settings = {{ATTRIBUTES = (RemoveHeadersOnCopy, ); }}; }};
/* End PBXBuildFile section */

/* Begin PBXContainerItemProxy section */
		{i(0xC01)} /* PBXContainerItemProxy */ = {{
			isa = PBXContainerItemProxy;
			containerPortal = {i(0x001)} /* Project object */;
			proxyType = 1;
			remoteGlobalIDString = {i(0x702)};
			remoteInfo = NotificationService;
		}};
/* End PBXContainerItemProxy section */

/* Begin PBXCopyFilesBuildPhase section */
		{i(0x803)} /* Embed Foundation Extensions */ = {{
			isa = PBXCopyFilesBuildPhase;
			buildActionMask = 2147483647;
			dstPath = "";
			dstSubfolderSpec = 13;
			files = (
				{i(0x103)} /* NotificationService.appex in Embed Foundation Extensions */,
			);
			name = "Embed Foundation Extensions";
			runOnlyForDeploymentPostprocessing = 0;
		}};
/* End PBXCopyFilesBuildPhase section */

/* Begin PBXFileReference section */
		{i(0x301)} /* BipAgents.app */ = {{isa = PBXFileReference; explicitFileType = wrapper.application; includeInIndex = 0; path = BipAgents.app; sourceTree = BUILT_PRODUCTS_DIR; }};
		{i(0x303)} /* NotificationService.appex */ = {{isa = PBXFileReference; explicitFileType = "wrapper.app-extension"; includeInIndex = 0; path = NotificationService.appex; sourceTree = BUILT_PRODUCTS_DIR; }};
		{i(0x302)} /* Info.plist */ = {{isa = PBXFileReference; lastKnownFileType = text.plist.xml; path = Info.plist; sourceTree = "<group>"; }};
		{i(0x304)} /* NotificationService-Info.plist */ = {{isa = PBXFileReference; lastKnownFileType = text.plist.xml; path = "NotificationService-Info.plist"; sourceTree = "<group>"; }};
		{i(0x305)} /* BipAgents.entitlements */ = {{isa = PBXFileReference; lastKnownFileType = text.plist.entitlements; path = BipAgents.entitlements; sourceTree = "<group>"; }};
		{i(0x306)} /* NotificationService.entitlements */ = {{isa = PBXFileReference; lastKnownFileType = text.plist.entitlements; path = NotificationService.entitlements; sourceTree = "<group>"; }};
/* End PBXFileReference section */

/* Begin PBXFileSystemSynchronizedRootGroup section */
		{i(0x401)} /* App */ = {{isa = PBXFileSystemSynchronizedRootGroup; path = App; sourceTree = "<group>"; }};
		{i(0x402)} /* NotificationService */ = {{isa = PBXFileSystemSynchronizedRootGroup; path = NotificationService; sourceTree = "<group>"; }};
/* End PBXFileSystemSynchronizedRootGroup section */

/* Begin PBXFrameworksBuildPhase section */
		{i(0x501)} /* Frameworks */ = {{
			isa = PBXFrameworksBuildPhase;
			buildActionMask = 2147483647;
			files = (
				{i(0x101)} /* HermesKit in Frameworks */,
				{i(0x102)} /* VoiceKit in Frameworks */,
			);
			runOnlyForDeploymentPostprocessing = 0;
		}};
		{i(0x502)} /* Frameworks */ = {{
			isa = PBXFrameworksBuildPhase;
			buildActionMask = 2147483647;
			files = (
			);
			runOnlyForDeploymentPostprocessing = 0;
		}};
/* End PBXFrameworksBuildPhase section */

/* Begin PBXGroup section */
		{i(0x601)} = {{
			isa = PBXGroup;
			children = (
				{i(0x401)} /* App */,
				{i(0x402)} /* NotificationService */,
				{i(0x603)} /* Config */,
				{i(0x602)} /* Products */,
			);
			sourceTree = "<group>";
		}};
		{i(0x602)} /* Products */ = {{
			isa = PBXGroup;
			children = (
				{i(0x301)} /* BipAgents.app */,
				{i(0x303)} /* NotificationService.appex */,
			);
			name = Products;
			sourceTree = "<group>";
		}};
		{i(0x603)} /* Config */ = {{
			isa = PBXGroup;
			children = (
				{i(0x302)} /* Info.plist */,
				{i(0x304)} /* NotificationService-Info.plist */,
				{i(0x305)} /* BipAgents.entitlements */,
				{i(0x306)} /* NotificationService.entitlements */,
			);
			path = Config;
			sourceTree = "<group>";
		}};
/* End PBXGroup section */

/* Begin PBXNativeTarget section */
		{i(0x701)} /* BipAgents */ = {{
			isa = PBXNativeTarget;
			buildConfigurationList = {i(0x902)};
			buildPhases = (
				{i(0x801)} /* Sources */,
				{i(0x501)} /* Frameworks */,
				{i(0x802)} /* Resources */,
				{i(0x803)} /* Embed Foundation Extensions */,
			);
			buildRules = (
			);
			dependencies = (
				{i(0xD01)} /* PBXTargetDependency */,
			);
			fileSystemSynchronizedGroups = (
				{i(0x401)} /* App */,
			);
			name = BipAgents;
			packageProductDependencies = (
				{i(0x201)} /* HermesKit */,
				{i(0x202)} /* VoiceKit */,
			);
			productName = BipAgents;
			productReference = {i(0x301)} /* BipAgents.app */;
			productType = "com.apple.product-type.application";
		}};
		{i(0x702)} /* NotificationService */ = {{
			isa = PBXNativeTarget;
			buildConfigurationList = {i(0x903)};
			buildPhases = (
				{i(0x804)} /* Sources */,
				{i(0x502)} /* Frameworks */,
				{i(0x805)} /* Resources */,
			);
			buildRules = (
			);
			dependencies = (
			);
			fileSystemSynchronizedGroups = (
				{i(0x402)} /* NotificationService */,
			);
			name = NotificationService;
			productName = NotificationService;
			productReference = {i(0x303)} /* NotificationService.appex */;
			productType = "com.apple.product-type.app-extension";
		}};
/* End PBXNativeTarget section */

/* Begin PBXProject section */
		{i(0x001)} /* Project object */ = {{
			isa = PBXProject;
			attributes = {{
				BuildIndependentTargetsInParallel = 1;
				LastSwiftUpdateCheck = 2700;
				LastUpgradeCheck = 2700;
				TargetAttributes = {{
					{i(0x701)} = {{
						CreatedOnToolsVersion = 27.0;
					}};
					{i(0x702)} = {{
						CreatedOnToolsVersion = 27.0;
					}};
				}};
			}};
			buildConfigurationList = {i(0x901)};
			developmentRegion = fr;
			hasScannedForEncodings = 0;
			knownRegions = (
				fr,
				en,
				Base,
			);
			mainGroup = {i(0x601)};
			minimizedProjectReferenceProxies = 1;
			packageReferences = (
				{i(0xA01)} /* XCLocalSwiftPackageReference "Packages/HermesKit" */,
				{i(0xA02)} /* XCLocalSwiftPackageReference "Packages/VoiceKit" */,
			);
			preferredProjectObjectVersion = 77;
			productRefGroup = {i(0x602)} /* Products */;
			projectDirPath = "";
			projectRoot = "";
			targets = (
				{i(0x701)} /* BipAgents */,
				{i(0x702)} /* NotificationService */,
			);
		}};
/* End PBXProject section */

/* Begin PBXResourcesBuildPhase section */
		{i(0x802)} /* Resources */ = {{isa = PBXResourcesBuildPhase; buildActionMask = 2147483647; files = (); runOnlyForDeploymentPostprocessing = 0; }};
		{i(0x805)} /* Resources */ = {{isa = PBXResourcesBuildPhase; buildActionMask = 2147483647; files = (); runOnlyForDeploymentPostprocessing = 0; }};
/* End PBXResourcesBuildPhase section */

/* Begin PBXSourcesBuildPhase section */
		{i(0x801)} /* Sources */ = {{isa = PBXSourcesBuildPhase; buildActionMask = 2147483647; files = (); runOnlyForDeploymentPostprocessing = 0; }};
		{i(0x804)} /* Sources */ = {{isa = PBXSourcesBuildPhase; buildActionMask = 2147483647; files = (); runOnlyForDeploymentPostprocessing = 0; }};
/* End PBXSourcesBuildPhase section */

/* Begin PBXTargetDependency section */
		{i(0xD01)} /* PBXTargetDependency */ = {{
			isa = PBXTargetDependency;
			target = {i(0x702)} /* NotificationService */;
			targetProxy = {i(0xC01)} /* PBXContainerItemProxy */;
		}};
/* End PBXTargetDependency section */

/* Begin XCBuildConfiguration section */
{cfg(i(0xB01), "Debug", proj_debug)}{cfg(i(0xB02), "Release", proj_release)}{cfg(i(0xB03), "Debug", app)}{cfg(i(0xB04), "Release", app)}{cfg(i(0xB05), "Debug", nse)}{cfg(i(0xB06), "Release", nse)}/* End XCBuildConfiguration section */

/* Begin XCConfigurationList section */
		{i(0x901)} /* Build configuration list for PBXProject "BipAgents" */ = {{
			isa = XCConfigurationList;
			buildConfigurations = (
				{i(0xB01)} /* Debug */,
				{i(0xB02)} /* Release */,
			);
			defaultConfigurationIsVisible = 0;
			defaultConfigurationName = Release;
		}};
		{i(0x902)} /* Build configuration list for PBXNativeTarget "BipAgents" */ = {{
			isa = XCConfigurationList;
			buildConfigurations = (
				{i(0xB03)} /* Debug */,
				{i(0xB04)} /* Release */,
			);
			defaultConfigurationIsVisible = 0;
			defaultConfigurationName = Release;
		}};
		{i(0x903)} /* Build configuration list for PBXNativeTarget "NotificationService" */ = {{
			isa = XCConfigurationList;
			buildConfigurations = (
				{i(0xB05)} /* Debug */,
				{i(0xB06)} /* Release */,
			);
			defaultConfigurationIsVisible = 0;
			defaultConfigurationName = Release;
		}};
/* End XCConfigurationList section */

/* Begin XCLocalSwiftPackageReference section */
		{i(0xA01)} /* XCLocalSwiftPackageReference "Packages/HermesKit" */ = {{
			isa = XCLocalSwiftPackageReference;
			relativePath = Packages/HermesKit;
		}};
		{i(0xA02)} /* XCLocalSwiftPackageReference "Packages/VoiceKit" */ = {{
			isa = XCLocalSwiftPackageReference;
			relativePath = Packages/VoiceKit;
		}};
/* End XCLocalSwiftPackageReference section */

/* Begin XCSwiftPackageProductDependency section */
		{i(0x201)} /* HermesKit */ = {{
			isa = XCSwiftPackageProductDependency;
			productName = HermesKit;
		}};
		{i(0x202)} /* VoiceKit */ = {{
			isa = XCSwiftPackageProductDependency;
			productName = VoiceKit;
		}};
/* End XCSwiftPackageProductDependency section */
"""
out = "// !$*UTF8*$!\n{\n\tarchiveVersion = 1;\n\tclasses = {\n\t};\n\tobjectVersion = 77;\n\tobjects = {\n\n" + objs + f"\t}};\n\trootObject = {i(0x001)} /* Project object */;\n}}\n"
open("BipAgents.xcodeproj/project.pbxproj", "w").write(out)
print("written")
