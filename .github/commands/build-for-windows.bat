@ECHO OFF

cargo build --release
cp target\release\shutdown-on-lan.exe build\windows\shutdown-on-lan.exe

echo "Build Complete – starting packaging" 

cd build
cd windows

"%WIX%\bin\candle.exe" -ext WixFirewallExtension -ext WixUtilExtension -ext WixBalExtension Product.wxs
"%WIX%\bin\light.exe" -ext WixFirewallExtension -ext WixUtilExtension -ext WixBalExtension Product.wixobj

echo "Downloading vcredist 2015"

curl https://download.microsoft.com/download/9/3/F/93FCF1E7-E6A4-478B-96E7-D4B285925B00/vc_redist.x86.exe --output vc_redist.x86.exe
curl https://download.microsoft.com/download/9/3/F/93FCF1E7-E6A4-478B-96E7-D4B285925B00/vc_redist.x64.exe --output vc_redist.x64.exe

echo "Packaging Installer"

"%WIX%\bin\candle.exe" -ext WixFirewallExtension -ext WixUtilExtension -ext WixBalExtension Bundle.wxs
"%WIX%\bin\light.exe" -ext WixFirewallExtension -ext WixUtilExtension -ext WixBalExtension Bundle.wixobj

