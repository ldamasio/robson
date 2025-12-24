FROM golang:1.23-alpine

WORKDIR /app

# Copy go mod and sum files
COPY go.mod go.sum ./

# Download all dependencies. Dependencies will be cached if the go.mod and go.sum files are not changed
RUN go mod download

# Copy the source from the current directory to the Working Directory inside the container
COPY . .

# Build the Go app
RUN go build -o robson-cli .

# Command to run the executable
# Defaults to running the server, but can be overridden
CMD ["./robson-cli", "server", "--redis", "robsonRedis:6379", "--port", "8080"]
