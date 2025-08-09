class FaradayRateLimitMiddleware < Faraday::Middleware
  def initialize(app, limit:, interval:)
    super(app)
    @limit = limit
    @interval = interval
    @requests = []
  end

  def call(env)
    loop do
      sleep_time = synchronize do
        clean_old_requests
        if @requests.size < @limit
          @requests << Time.now
          nil
        else
          @interval - (Time.now - @requests.first).to_f
        end
      end

      break if sleep_time.nil?
      sleep(sleep_time) if sleep_time > 0
    end
    @app.call(env)
  end

  private

  def synchronize
    @mutex ||= Mutex.new
    @mutex.synchronize { yield }
  end

  def clean_old_requests
    @requests.reject! { |time| Time.now - time > @interval }
  end
end

Faraday::Request.register_middleware(rate_limit: FaradayRateLimitMiddleware)
