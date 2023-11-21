# Create relayer
http post -t application/json http://127.0.0.1:3000/1/relayer/create { "name": "My Relayer", "chainId": 31337 }

# Relayer id: 4df4464e-d0af-4354-b08a-5a78978ab6e6
# Address: 0x968420740442caccb5023b2220e79e811f5ca798

# Send a transaction
http post -t application/json http://127.0.0.1:3000/1/tx/send {
    "relayerId": "a2d426a9-5b55-4048-812a-ef9e9f2b3a53",
    "to": "0x14bf69c64d27e5a5b07188b2e19f7501baa79209",
    "value": "10",
    "data": ""
    "gasLimit": "150000"
}

http get http://127.0.0.1:3000/1/tx/682b734b-87d0-41ee-9210-15fcbe13cc3d

for i in 0..100 {
    http post -t application/json http://127.0.0.1:3000/1/tx/send {
        "relayerId": "34e611c6-9b17-463a-8ca9-cdb640381f38",
        "to": "0x5ce4a426963abbf4f941c1af48594445fad99322",
        "value": "10",
        "data": ""
        "gasLimit": "150000"
    }
}
