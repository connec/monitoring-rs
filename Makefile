# Makefile
.PHONY: build-monitoring monitoring writer inspect rotate down reset push

DOCKER_IMAGE := registry.digitalocean.com/connec-co-uk/monitoring-rs:latest

build-monitoring:
	@docker-compose build monitoring

monitoring: build-monitoring
	@docker-compose up --force-recreate monitoring

writer:
	@docker-compose up -d writer

inspect:
	@docker-compose up inspect

rotate:
	@docker-compose up rotate

down:
	@docker-compose down --timeout 0 --volumes

reset: down writer

push: build-monitoring
	@docker push $(DOCKER_IMAGE)

kuberun: push
	@kubectl run monitoring-rs \
	    --image $(DOCKER_IMAGE) \
	    --env RUST_LOG=monitoring_rs=info \
	    --restart Never \
	    --dry-run=client \
	    --output json \
	  | jq '.spec.containers[0].volumeMounts |= [{ "name":"varlog", "mountPath":"/var/log", "readOnly":true }, { "name":"varlibdockercontainers", "mountPath":"/var/lib/docker/containers", "readOnly":true }]' \
	  | jq '.spec.volumes |= [{ "name":"varlog", "hostPath": { "path":"/var/log", "type":"Directory" }}, { "name":"varlibdockercontainers", "hostPath": { "path": "/var/lib/docker/containers", "type": "Directory" }}]' \
	  | kubectl run monitoring-rs \
	    --image $(DOCKER_IMAGE) \
	    --restart Never \
	    --overrides "$$(cat)"
	@kubectl wait --for=condition=Ready pod/monitoring-rs
	@kubectl logs -f monitoring-rs

kubecleanup:
	@kubectl delete pods monitoring-rs --ignore-not-found
