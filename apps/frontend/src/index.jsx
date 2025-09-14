import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import './index.css';
import './assets/bootstrap.min.css';

console.log('VITE_API_BASE_URL:', import.meta.env.VITE_API_BASE_URL);
console.log('Modo Vite:', import.meta.env.MODE);

const root = ReactDOM.createRoot(document.getElementById('root'));
root.render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
