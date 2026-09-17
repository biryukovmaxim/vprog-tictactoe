import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import { DaProvider } from './state';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <DaProvider>
      <App />
    </DaProvider>
  </StrictMode>,
);
