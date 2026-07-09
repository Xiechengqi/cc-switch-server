import { QueryClient } from "@tanstack/react-query";

import { isNonRetryableHttpError } from "@/lib/query/httpQueryUtils";

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: (failureCount, error) => {
        if (isNonRetryableHttpError(error)) {
          return false;
        }
        return failureCount < 1;
      },
      refetchOnWindowFocus: true,
      staleTime: 0,
    },
    mutations: {
      retry: false,
    },
  },
});
