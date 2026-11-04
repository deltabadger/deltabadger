# Records, for rust/tests/rails_unrecorded.rs (A28-1, PLAN-A28 r3.3), what Rails decides about a placement's transport
# failure and the text a failed Transaction keeps of it: each exception is run through Client.network_failure (the one
# place Rails classifies one) and Exchange#ambiguous_placement_error?, or, when it raises, through the placement guard's
# own test (Bot::ExchangeUser#with_placement_guard: ambiguous unless Client.pre_transmission?). Offline, no database rows.
#   s=$(mktemp -d); env -u DATABASE_URL SKIP_TEST_DATABASE=true PRIMARY_DATABASE_URL=sqlite3:$s/p.sqlite3 \
#     QUEUE_DATABASE_URL=sqlite3:$s/q.sqlite3 CACHE_DATABASE_URL=sqlite3:$s/c.sqlite3 CABLE_DATABASE_URL=sqlite3:$s/w.sqlite3 \
#     sh -c 'bin/rails runner script/rust/schema_template.rb && bin/rails runner script/rust/rails_unrecorded.rb rust/tests/fixtures/rails_unrecorded_vectors.json'
require 'json'
require 'digest'

Net::HTTP.prepend(Module.new { def connect = raise("real connection to #{address}:#{port}") })

module RailsUnrecordedVectors
  module_function

  PORTED = %w[app/models/client.rb app/models/exchange.rb app/models/bot/exchange_user.rb
              app/models/bot/composition/order_setter.rb app/models/bot/order_creator.rb].freeze

  def wrapped(outer, inner_class, inner_message, outer_message = nil)
    raise inner_class, inner_message
  rescue StandardError
    begin
      raise outer, (outer_message || $!.message)
    rescue StandardError => e
      e
    end
  end

  def cases
    [
      wrapped(Faraday::TimeoutError, Net::ReadTimeout, 'Net::ReadTimeout'),
      wrapped(Faraday::ConnectionFailed, Errno::ECONNRESET, 'Connection reset by peer'),
      wrapped(Faraday::ConnectionFailed, Errno::ECONNREFUSED, 'connect(2) for 127.0.0.1:443', 'Failed to open TCP connection to api.example:443 (Connection refused - connect(2) for 127.0.0.1:443)'),
      wrapped(Faraday::ConnectionFailed, Net::OpenTimeout, 'execution expired', 'Failed to open TCP connection to api.example:443 (execution expired)'),
      wrapped(Faraday::ConnectionFailed, SocketError, 'getaddrinfo: nodename nor servname provided, or not known',
              'Failed to open TCP connection to api.example:443 (getaddrinfo: nodename nor servname provided, or not known)'),
      wrapped(Faraday::ConnectionFailed, EOFError, 'end of file reached'),
      wrapped(Faraday::SSLError, OpenSSL::SSL::SSLError, 'SSL_read: unexpected eof while reading'),
      wrapped(Faraday::TimeoutError, Net::WriteTimeout, 'Net::WriteTimeout')
    ]
  end

  # [the text a failed row keeps, Rails' verdict]. A raise is the placement guard's verdict on the same message.
  def vectors
    exchange = Exchanges::Binance.new
    cases.map do |error|
      result = Client.network_failure(error)
      { 'errors' => Array(result.errors), 'ambiguous' => exchange.ambiguous_placement_error?(result), 'path' => 'result' }
    rescue Client::TransientNetworkError => e
      { 'errors' => [e.message], 'ambiguous' => !Client.pre_transmission?(e.original_class, e.message), 'path' => 'raised' }
    end
  end
end

sources = RailsUnrecordedVectors::PORTED.to_h { |path| [path, Digest::SHA256.file(Rails.root.join(path)).hexdigest] }
vectors = RailsUnrecordedVectors.vectors
File.write(ARGV.fetch(0), "#{JSON.pretty_generate({ 'ported_sources' => sources, 'vectors' => vectors })}\n")
puts "wrote #{vectors.size} vectors"
