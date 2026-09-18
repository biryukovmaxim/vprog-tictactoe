import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import { LaneCarriersProvider } from './carriers';
import { DaProvider } from './state';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <DaProvider>
      <LaneCarriersProvider>
        <App />
      </LaneCarriersProvider>
    </DaProvider>
  </StrictMode>,
);
