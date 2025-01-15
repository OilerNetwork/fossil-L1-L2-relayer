#!/bin/bash

set -e

# Parse command line arguments
VERBOSE=""
if [[ " $* " =~ " --verbose " ]] || [[ " $* " =~ " -v " ]]; then
    VERBOSE="--progress=plain"
fi

# Define colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

# Ensure we're using buildx
docker buildx create --use --name fossil-builder || true

echo -e "${BLUE}Building Docker images...${NC}"

# Build each image
echo -e "${BLUE}Building anvil image...${NC}"
docker buildx build --load -f docker/Dockerfile.anvil -t anvil:latest . $VERBOSE

echo -e "${BLUE}Building katana image...${NC}"
docker buildx build --load -f docker/Dockerfile.katana -t fossil-katana:latest . $VERBOSE

echo -e "${BLUE}Building deploy image...${NC}"
docker buildx build --load -f docker/Dockerfile.deploy -t fossil-deploy:latest . $VERBOSE

echo -e "${BLUE}Building build-mmr image...${NC}"
docker buildx build --load -f docker/Dockerfile.build-mmr -t fossil-build-mmr:latest . $VERBOSE

echo -e "${BLUE}Building relayer image...${NC}"
docker buildx build --load -f docker/Dockerfile.relayer -t fossil-relayer:latest . $VERBOSE

echo -e "${BLUE}Building client image...${NC}"
docker buildx build --load -f docker/Dockerfile.client -t fossil-client:latest . $VERBOSE

echo -e "${BLUE}Building state-proof-api image...${NC}"
docker buildx build -f docker/Dockerfile.api \
  --build-arg BINARY=state-proof-api \
  -t fossil-state-proof-api:latest \
  --load \
  . $VERBOSE

echo -e "${BLUE}Building fetch-fees-proof image...${NC}"
docker buildx build -f docker/Dockerfile.api \
  --build-arg BINARY=fetch-fees-proof \
  -t fossil-light-client/fetch-fees-proof \
  --load \
  .

# Clean up the builder
echo -e "${BLUE}Cleaning up builder...${NC}"
docker buildx rm fossil-builder

echo -e "${GREEN}All images built successfully!${NC}" 