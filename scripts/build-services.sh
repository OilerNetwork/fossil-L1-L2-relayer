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

echo -e "${BLUE}Building service Docker images...${NC}"

# Build service images
echo -e "${BLUE}Building build-mmr image...${NC}"
docker buildx build --load -f docker/Dockerfile.build-mmr -t fossil-build-mmr:latest . $VERBOSE

echo -e "${BLUE}Building relayer image...${NC}"
docker buildx build --load -f docker/Dockerfile.relayer -t fossil-relayer:latest . $VERBOSE

echo -e "${BLUE}Building client image...${NC}"
docker buildx build --load -f docker/Dockerfile.client -t fossil-client:latest . $VERBOSE

echo -e "${BLUE}Building state-proof-api image...${NC}"
docker buildx build --load -f docker/Dockerfile.api \
  --build-arg BINARY=state-proof-api \
  -t fossil-state-proof-api:latest \
  . $VERBOSE

echo -e "${BLUE}Building fetch-fees-proof image...${NC}"
docker buildx build --load -f docker/Dockerfile.api \
  --build-arg BINARY=fetch-fees-proof \
  -t fossil-fetch-fees-proof:latest \
  . $VERBOSE

# Clean up the builder
echo -e "${BLUE}Cleaning up builder...${NC}"
docker buildx rm fossil-builder

echo -e "${GREEN}Service images built successfully!${NC}" 