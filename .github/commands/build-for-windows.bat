@ECHO OFF

cargo build --release
cp target\release\shutdown-on-lan.exe build\windows\shutdown-on-lan.exe

echo "Build Complete – starting packaging" 

cd build
cd windows

"%WIX%\bin\candle.exe" -ext WixFirewallExtension Product.wxs
"%WIX%\bin\light.exe" -ext WixFirewallExtension Product.wixobj

mv Product.msi shutdown-on-lan.msi
