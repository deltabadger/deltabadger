# Record the actual job completion payloads, without network or persistent writes.
require 'digest'

def stubbed(object, name, value)
  original = object.method(name)
  object.define_singleton_method(name) { |*args, **kwargs| value.respond_to?(:call) ? value.call(*args, **kwargs) : value }
  yield
ensure
  object.define_singleton_method(name, original)
end

records = {}
[1, 42].each do |owner|
  user = Struct.new(:id, :wash_sale_days).new(owner, 0)
  key = Struct.new(:user_id, :user) do
    def record_sync_error!(_error); end
  end.new(owner, user)
  [false, true].each do |failed|
    messages = []
    sync = Object.new
    sync.define_singleton_method(:sync!) do
      raise 'recorded venue failure' if failed

      Struct.new(:failure?).new(false)
    end
    job = AccountTransaction::SyncJob.new
    job.define_singleton_method(:sleep) { |_| nil }
    job.define_singleton_method(:broadcast_sync_warnings) { |_| nil }
    stubbed(AccountTransactionSync, :new, sync) do
      stubbed(TransferMatcher, :run!, nil) do
        stubbed(Tracker::LedgerJob, :perform_later, nil) do
          stubbed(ActionCable.server, :broadcast, ->(stream, payload) { messages << [stream, payload] }) do
            job.perform(key)
          rescue RuntimeError => e
            raise unless failed && e.message == 'recorded venue failure'
          end
        end
      end
    end
    raise 'sync completion must broadcast exactly once' unless messages.size == 1

    records["sync_#{owner}_#{failed ? 'failure' : 'success'}"] = messages
  end
  messages = []
  stubbed(User, :find, user) do
    stubbed(Tracker::Ledger, :compute!, { nil => nil }) do
      stubbed(PortfolioSnapshot, :record!, nil) do
        stubbed(ActionCable.server, :broadcast, ->(stream, payload) { messages << [stream, payload] }) do
          Tracker::LedgerJob.new.perform(owner)
        end
      end
    end
  end
  raise 'ledger completion must broadcast exactly once' unless messages.size == 1

  records["ledger_#{owner}"] = messages
end
sources = %w[app/jobs/account_transaction/sync_job.rb app/jobs/tracker/ledger_job.rb].to_h do |path|
  [path, Digest::SHA256.file(Rails.root.join(path)).hexdigest]
end
File.write(ARGV.fetch(0), "#{JSON.pretty_generate({ sources: sources, records: records })}\n")
