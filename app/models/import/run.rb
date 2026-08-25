# One import: read a file in a chosen format, work out which account each row belongs to, then hand
# the rows to the very writer the exchange sync uses.
#
# ROUTING is not a question for the user. A venue format knows its venue — a Binance export exists
# nowhere else, and Binance.US writes the same one — and our own export names the venue on every
# row. The user is asked exactly once: when they hold BOTH Binance accounts and the file cannot say
# which of the two it came from. Everywhere else an account picker would be a control with one
# answer whose wrong answer files a Binance history under Hyperliquid.
#
# There is no preview step, because there is nothing on one to decide. The format is chosen before
# the file is read, the account follows from the format or from the rows, and the time zone comes out
# of the file's own name — a wrong file is refused rather than half-imported, and importing the same
# file twice changes nothing. A screen with no choice on it is a click, not a safeguard.
#
# The one question that survives is which of two Binance accounts a file came from, and it is asked
# only when the user holds both. `import!` answers `ambiguous` and writes nothing in that case.
class Import::Run
  FORMATS = { 'deltabadger' => Import::DeltabadgerCsv, 'binance' => Import::BinanceCsv }.freeze
  DEFAULT_FORMAT = 'deltabadger'.freeze

  def self.reader_for(format) = FORMATS[format.to_s]

  # A file the parser can read but that yields nothing recognisable is a file in the wrong format —
  # a Deltabadger export fed to the Binance reader, say. Better refused than half-imported.
  class UnreadableFile < StandardError; end

  def initialize(user:, format:, text:, offset: nil, api_key: nil)
    @user = user
    @format = format
    @text = self.class.as_text(text)
    @offset = offset
    @chosen_key = api_key
  end

  # A file is bytes until someone decides otherwise, and this is where that is decided — once, for
  # every format, rather than in each reader. An upload arrives as ASCII-8BIT; `scrub` replaces
  # anything that is not valid UTF-8 rather than raising halfway through a five-year history, and
  # the byte order mark Binance opens with is dropped here so no reader has to know about it.
  def self.as_text(text)
    text.to_s.dup.force_encoding(Encoding::UTF_8).scrub.delete_prefix("\uFEFF")
  end

  def import! = outcome

  # The accounts a file in this format could belong to. Empty means the user has none, which is a
  # refusal rather than a question.
  def candidates
    types = reader_class.exchange_types
    return [] if types.nil?

    ApiKey.reading(@user.api_keys.includes(:exchange))
          .select { |key| types.any? { |type| key.exchange.is_a?(type) } }
  end

  private

  def outcome
    reader = reader_class.new(@text, offset: @offset)
    entries = reader.entries
    raise UnreadableFile if entries.empty? && reader.unrecognised.empty?

    routed, unrouted = route(entries)
    return { **EMPTY, error: :no_account_for_format } if routed.nil?
    return { **EMPTY, ambiguous: true, candidates: candidates } if routed == :ambiguous

    counts = write(routed)
    counts.merge(unrecognised: reader.unrecognised, unrouted: unrouted)
  rescue UnreadableFile, CSV::MalformedCSVError, ArgumentError => e
    { **EMPTY, error: e.is_a?(UnreadableFile) ? :unreadable : :malformed }
  end

  EMPTY = { imported: 0, duplicates: 0, skipped: 0, unrecognised: [], unrouted: [] }.freeze

  # [{ api_key => entries }, unrouted_exchange_names] — or nil when this format has no account here
  # at all, or :ambiguous when only the user can say which of two it is.
  def route(entries)
    return route_by_row(entries) if reader_class.exchange_types.nil?

    options = candidates
    return [nil, []] if options.empty?
    return [{ @chosen_key => entries }, []] if @chosen_key && options.include?(@chosen_key)
    return [:ambiguous, []] if options.size > 1

    [{ options.first => entries }, []]
  end

  # Our own export: each row names its venue, so each row finds its own account. A venue this
  # install never connected has no account to hang from — those rows are REPORTED rather than
  # quietly filed under whichever key happened to be first.
  def route_by_row(entries)
    keys = ApiKey.reading(@user.api_keys.includes(:exchange)).index_by { |key| key.exchange.name_id }
    routed = Hash.new { |hash, key| hash[key] = [] }
    unrouted = []
    entries.each do |entry|
      key = keys[entry[:exchange_name_id]]
      key ? routed[key] << entry : unrouted << entry[:exchange_name_id]
    end
    [routed, unrouted.compact.uniq.sort]
  end

  # One transaction across every account the file touches: a Deltabadger export naming three venues
  # either lands whole or not at all, rather than leaving a history two thirds imported.
  def write(routed)
    totals = { imported: 0, duplicates: 0, skipped: 0 }
    accounts = {}
    ActiveRecord::Base.transaction do
      routed.each do |api_key, entries|
        counts = AccountTransactionSync.new(api_key).store!(entries)
        accounts[api_key] = counts
        totals.each_key { |field| totals[field] += counts[field] }
      end
    end
    totals.merge(accounts: accounts)
  end

  def reader_class
    FORMATS.fetch(@format.to_s) { raise UnreadableFile }
  end
end
