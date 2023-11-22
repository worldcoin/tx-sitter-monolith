echo "Creating relayer"
let relayer = http post -t application/json http://127.0.0.1:3000/1/relayer/create { "name": "My Relayer", "chainId": 31337 }

echo "Funding relayer"
cast send --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 --value 100ether $relayer.address ''

echo "Sending transaction"
let tx = http post -t application/json http://127.0.0.1:3000/1/tx/send {
    "relayerId": $relayer.relayerId,
    "to": $relayer.address,
    "value": "10",
    "data": ""
    "gasLimit": "150000"
}

echo "Wait until tx is mined"
for i in 0..100 {
    let txResponse = http get http://127.0.0.1:3000/1/tx/($tx.txId)

    if ($txResponse | get -i status) == "mined" {
        echo $txResponse
        break
    } else {
        sleep 1sec
    }
}

echo "Success!"
