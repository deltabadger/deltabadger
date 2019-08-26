import axios from 'axios';

const API_URL = '/api';

const element = document.getElementsByName('csrf-token')[0];
const token = element && element.getAttribute('content');

const client = axios.create({
  headers: { 'X-CSRF-Token': token },
});

const API = {
  get(endpoint, params) {
    const url = `${API_URL}/${endpoint}`;
    return client.request({ url, params, method: 'get' }).then(data => data.data);
  },

  post(endpoint, params) {
    return client.post(`/${endpoint}`, params).then(data => data.data);
  },

  getExchanges() {
    const url = `${API_URL}/exchanges`;
    return client.request({ url, params: {}, method: 'get' }).then(data => data.data);
  },

  createApiKey(params) {
    const url = `${API_URL}/api_keys`;
    const ApiKeyParams = {
      exchange_id: params.exchangeId,
      key: params.key
    }
    return client.request({ url, data: { api_key: ApiKeyParams}, method: 'post' }).then(data => data.data);
  }
};

export default API;
