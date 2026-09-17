$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or
    $env:GITHUB_REPOSITORY -ne 'CMMUU/serylane' -or $env:RUNNER_OS -ne 'Windows') {
    throw 'Package identity acceptance is restricted to disposable Serylane GitHub-hosted Windows runners.'
}
$name = 'Serylane.BindingFixture'
if (Get-AppxPackage -Name $name) { throw 'An existing fixture package is present; leave it untouched.' }
$root = Join-Path ([IO.Path]::GetTempPath()) ('serylane-binding-' + [guid]::NewGuid())
$publisher = 'CN=Serylane Binding Fixture'
$certificate = $null
$architecture = if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64') { 'arm64' } else { 'x64' }
# A loose Add-AppxPackage -Register directory resolves through PackageManager,
# but direct CreateProcess does not acquire its package identity. Use actual
# signed, installed MSIX packages to test the same launch path as Store apps.
# The short-lived test certificate never leaves this disposable CI runner.
$sdkRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
$sdk = Get-ChildItem -LiteralPath $sdkRoot -Directory | Where-Object {
    $_.Name -match '^10\.0\.\d+\.\d+$' -and
    (Test-Path -LiteralPath (Join-Path $_.FullName "$architecture\makeappx.exe")) -and
    (Test-Path -LiteralPath (Join-Path $_.FullName "$architecture\signtool.exe"))
} | Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1
if (-not $sdk) { throw 'Native Windows SDK package/signing tools were not found.' }
$makeAppx = Join-Path $sdk.FullName "$architecture\makeappx.exe"
$signTool = Join-Path $sdk.FullName "$architecture\signtool.exe"
try {
    New-Item -ItemType Directory -Path $root -Force | Out-Null
    $certificate = New-SelfSignedCertificate -Type Custom -KeyUsage DigitalSignature -CertStoreLocation 'Cert:\CurrentUser\My' -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}') -Subject $publisher -NotAfter (Get-Date).AddDays(1)
    $publicCertificate = Join-Path $root 'fixture.cer'
    Export-Certificate -Cert $certificate -FilePath $publicCertificate | Out-Null
    Import-Certificate -FilePath $publicCertificate -CertStoreLocation 'Cert:\LocalMachine\TrustedPeople' | Out-Null
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
        # Cargo unit-test binaries can have an external Common-Controls v6
        # activation manifest. Relocate it with the executable; otherwise the
        # loader can exit before Rust runs, producing no stdout/stderr.
        if (Test-Path -LiteralPath "$testBinary.manifest") {
            Copy-Item -LiteralPath "$testBinary.manifest" -Destination (Join-Path $directory 'app.exe.manifest')
        }
        Copy-Item 'assets/brand/app-icon-128.png' (Join-Path $directory 'Assets\Logo.png')
        $manifest = @"
<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10" xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10" xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities" IgnorableNamespaces="uap rescap">
 <Identity Name="$name" Publisher="$publisher" Version="$version" ProcessorArchitecture="$architecture" />
 <Properties><DisplayName>Serylane Binding Fixture</DisplayName><PublisherDisplayName>Serylane CI</PublisherDisplayName><Logo>Assets\Logo.png</Logo></Properties>
 <Dependencies><TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.17763.0" MaxVersionTested="10.0.26100.0" /></Dependencies>
 <Resources><Resource Language="en-us" /></Resources>
 <Applications><Application Id="App" Executable="app.exe" EntryPoint="Windows.FullTrustApplication"><uap:VisualElements DisplayName="Serylane Binding Fixture" Description="Disposable upgrade test" BackgroundColor="transparent" Square150x150Logo="Assets\Logo.png" Square44x44Logo="Assets\Logo.png" /></Application></Applications>
 <Capabilities><rescap:Capability Name="runFullTrust" /></Capabilities>
</Package>
"@
        $manifestPath = Join-Path $directory 'AppxManifest.xml'
        [IO.File]::WriteAllText($manifestPath, $manifest, [Text.UTF8Encoding]::new($false))
        $packageFile = Join-Path $root "fixture-$version.msix"
        & $makeAppx pack /d $directory /p $packageFile /o
        if ($LASTEXITCODE -ne 0) { throw 'Fixture package construction failed.' }
        & $signTool sign /fd SHA256 /sha1 $certificate.Thumbprint /s My $packageFile
        if ($LASTEXITCODE -ne 0) { throw 'Fixture package signing failed.' }
        Add-AppxPackage -Path $packageFile -ForceUpdateFromAnyVersion
        $package = Get-AppxPackage -Name $name
        if (-not $package -or $package.Version.ToString() -ne $version) { throw 'Fixture registration version mismatch.' }
        $env:SERYLANE_BINDING_TEST_FAMILY = $package.PackageFamilyName
        $env:SERYLANE_BINDING_TEST_VERSION = $version
        $env:SERYLANE_BINDING_TEST_ROOT = $package.InstallLocation
        & $testBinary --exact program_proxy::tests::registered_app_upgrade_receives_proxy_environment --ignored --nocapture
        if ($LASTEXITCODE -ne 0) { throw "Registered package launch/upgrade contract failed for $version." }
        Write-Host "Verified registered package version $version with child-only proxy environment."
    }
} finally {
    Get-AppxPackage -Name $name | Where-Object { $_.Publisher -eq $publisher } | Remove-AppxPackage
    foreach ($variable in @('SERYLANE_BINDING_TEST_FAMILY', 'SERYLANE_BINDING_TEST_VERSION', 'SERYLANE_BINDING_TEST_ROOT')) {
        Remove-Item -LiteralPath "Env:\$variable" -ErrorAction SilentlyContinue
    }
    if ($certificate) {
        foreach ($store in @('Cert:\CurrentUser\My', 'Cert:\LocalMachine\TrustedPeople')) {
            $ownedCertificate = Join-Path $store $certificate.Thumbprint
            if (Test-Path -LiteralPath $ownedCertificate) { Remove-Item -LiteralPath $ownedCertificate }
        }
    }
    $resolvedRoot = [IO.Path]::GetFullPath($root)
    $expectedParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
    if ([IO.Path]::GetDirectoryName($resolvedRoot) -ne $expectedParent -or [IO.Path]::GetFileName($resolvedRoot) -notlike 'serylane-binding-*') { throw 'Refusing cleanup outside the exact temporary fixture directory.' }
    if (Test-Path -LiteralPath $resolvedRoot) { Remove-Item -LiteralPath $resolvedRoot -Recurse -Force }
}
