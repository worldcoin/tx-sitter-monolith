# Setup dependencies in different terminals:
# DB
psql postgres://postgres:postgres@127.0.0.1:5432/database

# Nodes
anvil --chain-id 31337 -p 8545 --block-time 1
anvil --chain-id 31338 -p 8546 --block-time 1

# TxSitter
cargo watch -x run
# or just
cargo run

let txSitter = "http://127.0.0.1:3000"
let anvilSocket = "127.0.0.1:8545"
let anvilSocket2 = "127.0.0.1:8546"

http post -t application/json $"($txSitter)/1/network/31337" {
    name: "Anvil network",
    httpRpc: $"http://($anvilSocket)",
    wsRpc: $"ws://($anvilSocket)"
}

http post -t application/json $"($txSitter)/1/network/31338" {
    name: "Secondary Anvil network",
    httpRpc: $"http://($anvilSocket2)",
    wsRpc: $"ws://($anvilSocket2)"
}

echo "Creating relayer"
let relayer = http post -t application/json $"($txSitter)/1/relayer" { "name": "My Relayer", "chainId": 31337 }

echo "Update relayer - with secondary chain gas limit dependency"
http post -t application/json $"($txSitter)/1/relayer/($relayer.relayerId)" {
    gasLimits: [
        {
            chainId: 31338,
            # Note that this value is hexadecimal
            value: "3B9ACA00"
        }
    ]
}

echo "Funding relayer"
cast send --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 --value 100ether $relayer.address ''

echo "Sending transaction"
let tx = http post -t application/json $"($txSitter)/1/tx/send" {
    "relayerId": $relayer.relayerId,
    "to": $relayer.address,
    "value": "10",
    "data": ""
    "gasLimit": "150000"
}

echo "Wait until tx is mined"
for i in 0..100 {
    let txResponse = http get $"($txSitter)/1/tx/($tx.txId)"

    if ($txResponse | get -i status) == "mined" {
        echo $txResponse
        break
    } else {
        sleep 1sec
    }
}

echo "Success!"
