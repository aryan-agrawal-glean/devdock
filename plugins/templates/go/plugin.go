package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
)

type Message struct {
	Method string                 `json:"method"`
	Config map[string]interface{} `json:"config,omitempty"`
	Event  map[string]interface{} `json:"event,omitempty"`
}

type Response struct {
	Status  string `json:"status"`
	Message string `json:"message,omitempty"`
}

func main() {
	scanner := bufio.NewScanner(os.Stdin)
	for scanner.Scan() {
		var msg Message
		if err := json.Unmarshal(scanner.Bytes(), &msg); err != nil {
			respond("error", err.Error())
			continue
		}

		switch msg.Method {
		case "initialize":
			// YOUR CODE HERE
			respond("ok", "")
		case "emit":
			// YOUR CODE HERE
			fmt.Fprintf(os.Stderr, "[%s] %v\n", msg.Event["type"], msg.Event)
			respond("ok", "")
		case "shutdown":
			respond("ok", "")
			return
		default:
			respond("error", "unknown method: "+msg.Method)
		}
	}
}

func respond(status, message string) {
	resp := Response{Status: status, Message: message}
	data, _ := json.Marshal(resp)
	fmt.Println(string(data))
}
