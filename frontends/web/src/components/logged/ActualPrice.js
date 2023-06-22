import React, { useState, useEffect } from 'react'
import { Card } from 'react-bootstrap'
import axios from 'axios';
import useWebSocket from 'react-use-websocket';

function ActualPrice() {

  const [numero,setNumero] = useState(0);

  const { lastJsonMessage, sendMessage } = useWebSocket('wss://stream.binance.com:9443/ws/btcusdt@ticker', {
    onOpen: () => console.log(`Connected to App WS`),
    onMessage: () => {
      if (lastJsonMessage) {
        setNumero(lastJsonMessage.a);
      }
    },
    // queryParams: { 'token': '123456' },
    onError: (event) => { console.error(event); },
    shouldReconnect: (closeEvent) => true,
    reconnectInterval: 3000
  });

  return (
        <div>
          {numero}
        </div>
   )

}



export default ActualPrice
