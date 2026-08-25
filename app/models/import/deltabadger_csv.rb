# Our own export, read back. The same columns `AccountTransaction#to_csv_row` writes, in the same
# order — one format shared by both directions, so a file that leaves the app can always return to
# it. It carries `tx_id`, which is what lets a re-import recognise every row exactly rather than by
# resemblance; the venue exports have no such thing.
class Import::DeltabadgerCsv
  attr_reader :unrecognised

  # Not bound to a venue: every row names its own, so the file routes itself and there is nothing
  # to ask. `nil` is the difference between "the format knows" and "the rows know".
  def self.exchange_types = nil

  # Our own export writes an INSTANT (`...Z`), so there is no zone to supply and none to get wrong.
  def self.requires_offset? = false

  def initialize(text, offset: nil)
    # Bytes become text in ONE place, so a byte order mark or a stray non-UTF-8 byte cannot
    # trip a reader constructed directly rather than through the run.
    @text = Import::Run.as_text(text)
    @offset = offset
    @unrecognised = []
  end

  def entries
    @entries ||= CSV.parse(@text, headers: true).filter_map do |row|
      type = row['type'].to_s.strip
      unless AccountTransaction.entry_types.key?(type)
        @unrecognised << type unless type.blank? || @unrecognised.include?(type)
        next
      end

      { exchange_name_id: row['exchange'].to_s.strip.presence,
        entry_type: type.to_sym,
        base_currency: row['base_currency'], base_amount: row['base_amount'].to_d,
        quote_currency: row['quote_currency'].presence,
        quote_amount: row['quote_amount'].presence&.to_d,
        fee_currency: row['fee_currency'].presence,
        fee_amount: row['fee_amount'].presence&.to_d,
        # Our own dates are written with a zone (`...Z`); an offset chosen in the dialog is for
        # files that state a bare wall clock, so it is deliberately not applied here.
        tx_id: row['tx_id'].presence, group_id: row['group_id'].presence,
        description: row['description'].presence,
        transacted_at: Time.parse(row['date'].to_s).utc, raw_data: {} }
    end.sort_by { |entry| entry[:transacted_at] }
  end
end
