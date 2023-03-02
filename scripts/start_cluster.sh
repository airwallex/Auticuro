set -x

# Change dir to firm-wallet-service
cd firm-wallet-service

# Start 5 peers
sh -c 'sh run_node.sh 1 &
 sh run_node.sh 2 &
 sh run_node.sh 3 &
 sh run_node.sh 4 &
 sh run_node.sh 5 &
 wait'