set -x

cd firm-wallet-service

# 'Ctrl + C' will exit both the gateway and the wallet service cluster
(trap 'kill 0' SIGINT;
# Start gateway + 5 peers
sh -c ' cd ../firm-wallet-gateway && sh run_gateway.sh &
  sh run_node.sh 1 &
  sh run_node.sh 2 &
  sh run_node.sh 3 &
  sh run_node.sh 4 &
  sh run_node.sh 5 '
)

