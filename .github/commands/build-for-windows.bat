@ECHO OFF

cargo build --release
cp target\release\shutdown-on-lan.exe build\windows\shutdown-on-lan.exe

echo "Build Complete – starting packaging" 

cd build
cd windows

"%WIX%\bin\candle.exe" -ext WixFirewallExtension -ext WixUtilExtension Product.wxs
"%WIX%\bin\candle.exe" -ext WixFirewallExtension -ext WixUtilExtension Bundle.wxs
"%WIX%\bin\light.exe" -ext WixFirewallExtension -ext WixUtilExtension Product.wixobj

cp Product.msi shutdown-on-lan.msi

ls