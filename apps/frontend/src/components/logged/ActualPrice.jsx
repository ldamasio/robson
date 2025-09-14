import React, { useState, useEffect } from 'react'
import useWebSocket from 'react-use-websocket';

function ActualPrice() {

  const [numero,setNumero] = useState(0);

  const WS_URL = import.meta.env.VITE_WS_URL_BINANCE || 'wss://stream.binance.com:9443/ws/btcusdt@ticker'
  const { lastJsonMessage, sendMessage } = useWebSocket(WS_URL, {
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
