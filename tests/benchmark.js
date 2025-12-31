import http from 'k6/http';
import { check } from 'k6';

export const options = {
    discardResponseBodies: true, // we only care about throughput
    scenarios: {
        max_throughput: {
            executor: 'ramping-vus',
            startVUs: 10,
            stages: [
                { duration: '10s', target: 100 },
                { duration: '30s', target: 500 }, // Ramp to 500 VUs
                { duration: '30s', target: 1000 }, // Ramp to 1000 VUs
                { duration: '10s', target: 0 },
            ],
            gracefulStop: '5s',
        },
    },
};

export default function () {
    const payload = JSON.stringify({
        model: "gpt-3.5-turbo",
        messages: [{ role: "user", content: "Hello!" }]
    });

    const params = {
        headers: {
            'Content-Type': 'application/json',
        },
    };

    const port = __ENV.PORT || '8100';
    const res = http.post(`http://localhost:${port}/v1/chat/completions`, payload, params);

    check(res, {
        'status is 200': (r) => r.status === 200,
    });
}
