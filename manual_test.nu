# Create relayer
http post -t application/json http://127.0.0.1:3000/1/relayer/create { "name": "My Relayer", "chainId": 31337 }

# Relayer id: 4df4464e-d0af-4354-b08a-5a78978ab6e6
# Address: 0x968420740442caccb5023b2220e79e811f5ca798

# Send a transaction
http post -t application/json http://127.0.0.1:3000/1/tx/send {
    "relayerId": "2c1c949e-22f5-4c9a-adb7-f2a7b17111e1",
    "to": "0xe46e8850051fba970ce6108601df51abb460326f",
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
