# Get the leader id of the cluster

node_count=5 # Node count of the cluster

for store_id in `seq 1 $node_count`
do
    leader_gauge=`curl -s http://0.0.0.0:2021${store_id}/metrics | grep "is_leader_gauge 1" | wc -l`

    if [ ${leader_gauge} -eq 1 ]; then
        echo "Current Leader: ${store_id}"
        exit 0
    fi
done

echo "Found no leader, has the cluster been started?"
echo "If yes, wait for 3 secs and run this script again,"
echo "else, start the cluster by calling 'sh scripts/start_cluster.sh'"