param([switch]$AllowLocalFixture)
$ErrorActionPreference = 'Stop'
if (-not $env:GITHUB_ACTIONS -and -not $AllowLocalFixture) {
    throw 'This installs a disposable development package. Use a GitHub Windows runner or explicitly pass -AllowLocalFixture.'
}
$name = 'Serylane.BindingFixture'
if (Get-AppxPackage -Name $name) { throw 'An existing fixture package is present; leave it untouched.' }
$root = Join-Path ([IO.Path]::GetTempPath()) ('serylane-binding-' + [guid]::NewGuid())
$developmentKey = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock'
$previous = Get-ItemProperty -Path $developmentKey -Name AllowDevelopmentWithoutDevLicense -ErrorAction SilentlyContinue
$hadValue = $null -ne $previous
$previousValue = if ($hadValue) { $previous.AllowDevelopmentWithoutDevLicense } else { $null }
$architecture = if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64') { 'arm64' } else { 'x64' }
try {
    New-Item -ItemType Directory -Path $root -Force | Out-Null
    New-Item -Path $developmentKey -Force | Out-Null
    Set-ItemProperty -Path $developmentKey -Name AllowDevelopmentWithoutDevLicense -Type DWord -Value 1
    $build = & cargo test --manifest-path src-tauri/Cargo.toml --lib --no-run --message-format=json
    if ($LASTEXITCODE -ne 0) { throw 'Binding test build failed.' }
    $testBinary = $build | ForEach-Object { try { $_ | ConvertFrom-Json } catch {} } |
        Where-Object { $_.reason -eq 'compiler-artifact' -and $_.profile.test -and $_.executable -and $_.target.name -eq 'serylane_lib' } |
        Select-Object -Last 1 -ExpandProperty executable
    if (-not $testBinary) { throw 'Native library test executable was not produced.' }
    foreach ($version in @('1.0.0.0', '2.0.0.0')) {
        $directory = Join-Path $root "version-$version"
        New-Item -ItemType Directory -Path (Join-Path $directory 'Assets') -Force | Out-Null
        Copy-Item $testBinary (Join-Path $directory 'app.exe')
        Copy-Item 'assets/brand/app-icon-128.png' (Join-Path $directory 'Assets\Logo.png')
        $manifest = @"
<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10" xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10" xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities" IgnorableNamespaces="uap rescap">
 <Identity Name="$name" Publisher="CN=Serylane Binding Fixture" Version="$version" ProcessorArchitecture="$architecture" />
 <Properties><DisplayName>Serylane Binding Fixture</DisplayName><PublisherDisplayName>Serylane CI</PublisherDisplayName><Logo>Assets\Logo.png</Logo></Properties>
 <Dependencies><TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.17763.0" MaxVersionTested="10.0.26100.0" /></Dependencies>
 <Resources><Resource Language="en-us" /></Resources>
 <Applications><Application Id="App" Executable="app.exe" EntryPoint="Windows.FullTrustApplication"><uap:VisualElements DisplayName="Serylane Binding Fixture" Description="Disposable upgrade test" BackgroundColor="transparent" Square150x150Logo="Assets\Logo.png" Square44x44Logo="Assets\Logo.png" /></Application></Applications>
 <Capabilities><rescap:Capability Name="runFullTrust" /></Capabilities>
</Package>
"@
        $manifestPath = Join-Path $directory 'AppxManifest.xml'
        [IO.File]::WriteAllText($manifestPath, $manifest, [Text.UTF8Encoding]::new($false))
        # The old directory deliberately remains on disk. Resolution must select
        # the current user's registration, not the first/largest folder found.
        Add-AppxPackage -Register $manifestPath -ForceUpdateFromAnyVersion
        $package = Get-AppxPackage -Name $name
        if (-not $package -or $package.Version.ToString() -ne $version) { throw 'Fixture registration version mismatch.' }
        $env:SERYLANE_BINDING_TEST_FAMILY = $package.PackageFamilyName
        $env:SERYLANE_BINDING_TEST_VERSION = $version
        & $testBinary --exact program_proxy::tests::registered_app_upgrade_receives_proxy_environment --ignored --nocapture
        if ($LASTEXITCODE -ne 0) { throw "Registered package launch/upgrade contract failed for $version." }
        Write-Host "Verified registered package version $version with child-only proxy environment."
    }
} finally {
    Get-AppxPackage -Name $name | Remove-AppxPackage
    Remove-Item Env:\SERYLANE_BINDING_TEST_FAMILY -ErrorAction SilentlyContinue
    Remove-Item Env:\SERYLANE_BINDING_TEST_VERSION -ErrorAction SilentlyContinue
    if ($hadValue) { Set-ItemProperty -Path $developmentKey -Name AllowDevelopmentWithoutDevLicense -Value $previousValue }
    else { Remove-ItemProperty -Path $developmentKey -Name AllowDevelopmentWithoutDevLicense -ErrorAction SilentlyContinue }
    if (Test-Path $root) { Remove-Item $root -Recurse -Force }
}
