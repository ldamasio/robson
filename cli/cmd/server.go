package cmd

import (
	"context"
	"encoding/json"
	"log"
	"net/http"
	"sync"
	"time"

	"github.com/gorilla/websocket"
	"github.com/redis/go-redis/v9"
	"github.com/spf13/cobra"
)

var (
	redisAddr string
	wsPort    string
)

var serverCmd = &cobra.Command{
	Use:   "server",
	Short: "Start the real-time market data server",
	Long:  `Starts a WebSocket server that broadcasts market data updates from Redis Pub/Sub to connected clients.`,
	Run: func(cmd *cobra.Command, args []string) {
		runServer()
	},
}

func init() {
	serverCmd.Flags().StringVar(&redisAddr, "redis", "localhost:6379", "Redis address")
	serverCmd.Flags().StringVar(&wsPort, "port", "8080", "WebSocket server port")
	rootCmd.AddCommand(serverCmd)
}

// WebSocket upgrader
var upgrader = websocket.Upgrader{
	CheckOrigin: func(r *http.Request) bool {
		return true // Allow all origins for dev
	},
}

type MarketData struct {
	Symbol    string  `json:"symbol"`
	Price     float64 `json:"price"`
	Timestamp int64   `json:"timestamp"`
}

type Client struct {
	conn *websocket.Conn
	send chan []byte
}

type Hub struct {
	clients    map[*Client]bool
	broadcast  chan []byte
	register   chan *Client
	unregister chan *Client
	mu         sync.Mutex
}

func newHub() *Hub {
	return &Hub{
		broadcast:  make(chan []byte),
		register:   make(chan *Client),
		unregister: make(chan *Client),
		clients:    make(map[*Client]bool),
	}
}

func (h *Hub) run() {
	for {
		select {
		case client := <-h.register:
			h.mu.Lock()
			h.clients[client] = true
			h.mu.Unlock()
		case client := <-h.unregister:
			h.mu.Lock()
			if _, ok := h.clients[client]; ok {
				delete(h.clients, client)
				close(client.send)
			}
			h.mu.Unlock()
		case message := <-h.broadcast:
			h.mu.Lock()
			for client := range h.clients {
				select {
				case client.send <- message:
				default:
					close(client.send)
					delete(h.clients, client)
				}
			}
			h.mu.Unlock()
		}
	}
}

func runServer() {
	ctx := context.Background()
	hub := newHub()
	go hub.run()

	// Redis Client
	rdb := redis.NewClient(&redis.Options{
		Addr: redisAddr,
	})

	// 1. Data Publisher Routine (Mocking Binance fetch for now)
	// In production, this would be a separate service or consuming a real stream
	go func() {
		ticker := time.NewTicker(1 * time.Second)
		defer ticker.Stop()

		// Simulating price updates
		price := 50000.0

		for range ticker.C {
			// Simulate random price movement
			price += (float64(time.Now().UnixNano()%100) - 50.0) / 10.0

			data := MarketData{
				Symbol:    "BTCUSDC",
				Price:     price,
				Timestamp: time.Now().Unix(),
			}

			jsonBytes, _ := json.Marshal(data)

			// Publish to Redis
			err := rdb.Publish(ctx, "market_prices", jsonBytes).Err()
			if err != nil {
				log.Printf("Redis Publish Error: %v", err)
			}
		}
	}()

	// 2. Redis Subscriber Routine
	go func() {
		pubsub := rdb.Subscribe(ctx, "market_prices")
		defer pubsub.Close()

		ch := pubsub.Channel()
		for msg := range ch {
			hub.broadcast <- []byte(msg.Payload)
		}
	}()

	// 3. HTTP/WebSocket Server
	http.HandleFunc("/ws", func(w http.ResponseWriter, r *http.Request) {
		serveWs(hub, w, r)
	})

	log.Printf("Starting WebSocket server on :%s", wsPort)
	if err := http.ListenAndServe(":"+wsPort, nil); err != nil {
		log.Fatal("ListenAndServe: ", err)
	}
}

func serveWs(hub *Hub, w http.ResponseWriter, r *http.Request) {
	conn, err := upgrader.Upgrade(w, r, nil)
	if err != nil {
		log.Println(err)
		return
	}

	client := &Client{conn: conn, send: make(chan []byte, 256)}
	hub.register <- client

	// Allow collection of memory referenced by the caller by doing all work in
	// new goroutines.
	go client.writePump()
	go client.readPump(hub)
}

func (c *Client) readPump(hub *Hub) {
	defer func() {
		hub.unregister <- c
		c.conn.Close()
	}()
	for {
		_, _, err := c.conn.ReadMessage()
		if err != nil {
			break
		}
	}
}

func (c *Client) writePump() {
	defer func() {
		c.conn.Close()
	}()
	for {
		select {
		case message, ok := <-c.send:
			if !ok {
				c.conn.WriteMessage(websocket.CloseMessage, []byte{})
				return
			}
			c.conn.WriteMessage(websocket.TextMessage, message)
		}
	}
}
