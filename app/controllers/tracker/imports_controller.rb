# Reading a transaction file into the ledger.
#
# ONE step. There is nothing to decide on a second one: the format is chosen before the file is
# read, the account follows from that format or from the rows themselves, and the time zone comes
# out of the file's own name. A wrong file is refused rather than half-imported, and importing the
# same file twice changes nothing — so a confirmation screen would be a click, not a safeguard.
#
# The single exception is a question only the user can answer: which of two Binance accounts a file
# came from, asked only when they hold both. That is the one path that parks the upload on disk and
# comes back for an answer — the session is no place for a five-year export.
class Tracker::ImportsController < ApplicationController
  before_action :authenticate_user!

  UPLOAD_TTL = 1.hour
  TOKEN = /\A[a-f0-9]{16}\z/

  def new
    render :new, layout: false
  end

  def create
    @format = Import::Run::FORMATS.key?(params[:format].to_s) ? params[:format].to_s : Import::Run::DEFAULT_FORMAT
    @token, text, filename = fetch_upload
    return render_error(:no_file) if text.blank?

    @offset = resolve_offset(filename)
    return render_error(:unknown_timezone) if @offset == :unknown

    @result = Import::Run.new(user: current_user, format: @format, text: text,
                              offset: @offset, api_key: chosen_key).import!
    @result[:ambiguous] ? park(text) : release
    return imported! if @result[:imported].to_i.positive?

    # A file the reader could not make sense of, or has no account for, is a rejected submission —
    # the status is what tells Turbo to keep the dialog open on it.
    render :create, layout: false, status: @result[:error] ? :unprocessable_entity : :ok
  end

  private

  # Rows landed, so the dialog has nothing left to say — it closes, the page reloads with them on
  # it, and the flash carries the only sentence worth reading. Anything else would be a screen whose
  # single button is "I see".
  def imported!
    # Prepended AND redirected, in that order. `#flash` is `data-turbo-permanent`, so it is the one
    # element a visit does NOT replace — which is what carries the message across the reload, and
    # equally why a plain `flash[:notice]` on the next page would be thrown away.
    flash.now[:notice] = t('tracker.import.imported', count: @result[:imported])
    render turbo_stream: [turbo_stream_prepend_flash, turbo_stream_redirect(tracker_path)]
  end

  # The zone is a property of the FILE — Binance puts it in the name — and is never offered as a
  # field: the only edit a user could make to it is a wrong one, and a wrong one matches nothing
  # already stored, so the whole history lands again, hours out of place. A name that no longer
  # carries it is refused for the same reason.
  #
  # Parked beside the upload rather than round-tripped through the form, because the second request
  # (the Binance/Binance.US answer) carries a token, not a filename.
  def resolve_offset(filename)
    return nil unless Import::Run.reader_for(@format)&.requires_offset?
    return parked_offsets[@token] || :unknown if params[:token].present?

    offset = Import::BinanceCsv.offset_from(filename)
    offset || :unknown
  end

  # Only ever one of the accounts the run itself offered. A hand-crafted request naming a Hyperliquid
  # key for a Binance file gets nothing here, and the run then refuses the file outright rather than
  # filing a Binance history under another venue.
  def chosen_key
    return nil if params[:api_key_id].blank?

    ApiKey.reading(current_user.api_keys.includes(:exchange))
          .find { |key| key.id.to_s == params[:api_key_id].to_s }
  end

  # Only the unanswered question parks anything. Everything else has already been written by the
  # time we get here, so the bytes have no further use.
  def park(text)
    @token ||= SecureRandom.hex(8)
    FileUtils.mkdir_p(File.dirname(upload_path(@token)))
    File.binwrite(upload_path(@token), text)
    session[:import_offsets] = parked_offsets.merge(@token => @offset) if @offset
    sweep_old_uploads
  end

  def release
    return if @token.blank?

    FileUtils.rm_f(upload_path(@token))
    session[:import_offsets] = parked_offsets.except(@token)
  end

  # A fresh upload, or the file the unanswered question parked a moment ago.
  def fetch_upload
    if params[:token].to_s.match?(TOKEN)
      path = upload_path(params[:token])
      return [params[:token], nil, nil] unless File.exist?(path)

      return [params[:token], File.binread(path), nil]
    end

    file = params[:file]
    return [nil, nil, nil] if file.blank?

    # BINARY. An upload arrives as ASCII-8BIT and Binance's export opens with a UTF-8 byte order
    # mark, so anything text-mode tries to transcode those bytes and raises. `Import::Run` is what
    # decides they are text.
    [nil, file.read.b, file.original_filename.to_s]
  end

  def upload_path(token)
    Rails.root.join('tmp', 'imports', current_user.id.to_s, "#{token}.csv")
  end

  # A question nobody answered leaves a file behind. Swept on the next park rather than by a job:
  # there is no schedule worth adding for a directory only this action ever writes to.
  def sweep_old_uploads
    Dir.glob(upload_path('*')).each do |path|
      File.delete(path) if File.mtime(path) < UPLOAD_TTL.ago
    end
  end

  # Only the offsets whose files are still parked, so a stale cookie cannot outlive the upload.
  def parked_offsets
    (session[:import_offsets] || {}).slice(*Dir.glob(upload_path('*')).map { |path| File.basename(path, '.csv') })
  end

  def render_error(reason)
    @error = reason
    render :create, layout: false, status: :unprocessable_entity
  end
end
