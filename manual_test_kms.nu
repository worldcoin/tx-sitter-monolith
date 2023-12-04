## Setup dependencies in different terminals:
## DB
# docker run --rm -e POSTGRES_HOST_AUTH_METHOD=trust -p 5432:5432 postgres
## Can connect to using psql postgres://postgres:postgres@127.0.0.1:5432/database
## TxSitter
# cargo watch -x run
## or just
# cargo run

echo "Start"

let txSitter = "http://127.0.0.1:3000"

http post -t application/json $"($txSitter)/1/admin/network/11155111" {
    name: "Ethereum Sepolia",
    httpRpc: $env.SEPOLIA_HTTP_RPC,
    wsRpc: $env.SEPOLIA_WS_RPC,
}

echo "Creating relayer"
let relayer = http post -t application/json $"($txSitter)/1/admin/relayer" { "name": "My Relayer", "chainId": 11155111 }

http post -t application/json $"($txSitter)/1/admin/relayer/($relayer.relayerId)" {
    gasLimits: [
        { chainId: 11155111, value: "0x123" }
    ]
}

echo "Create api key"
let apiKey = http post $"($txSitter)/1/admin/relayer/($relayer.relayerId)/key" ""

$env.ETH_RPC_URL = $"($txSitter)/1/api/($apiKey.apiKey)/rpc"

echo "Funding relayer"
cast send --private-key $env.PRIVATE_KEY --value 1ether $relayer.address ''

echo "Sending transaction"
let tx = http post -t application/json $"($txSitter)/1/api/($apiKey.apiKey)/tx" {
    "relayerId": $relayer.relayerId,
    "to": $relayer.address,
    "value": "10",
    "data": ""
    "gasLimit": "150000"
}

echo "Wait until tx is mined"
for i in 0..100 {
    let txResponse = http get $"($txSitter)/1/api/($apiKey.apiKey)/tx/($tx.txId)"

    if ($txResponse | get -i status) == "mined" {
        echo $txResponse
        break
    } else {
        echo $txResponse
        sleep 1sec
    }
}

echo "Success!"
