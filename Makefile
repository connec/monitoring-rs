# Makefile
.PHONY: monitoring writer inspect rotate down reset

monitoring:
	@docker-compose up --build --force-recreate monitoring

writer:
	@docker-compose up -d writer

inspect:
	@docker-compose up inspect

rotate:
	@docker-compose up rotate

down:
	@docker-compose down --timeout 0 --volumes

reset: down writer
