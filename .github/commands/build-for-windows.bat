@ECHO OFF

cargo build --release
cp .\target\release\shutdown-on-lan.exe .\build\windows\shutdown-on-lan.exe

cd .\build\windows

tools\candle.exe -ext WixFirewallExtension Product.wxs
tools\light.exe -ext WixFirewallExtension Product.wixobj
