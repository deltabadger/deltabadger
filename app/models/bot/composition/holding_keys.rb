# The key each holding of a composition bot is known by — in the ledger, on the page, in the chart and
# through the API. A holding is one asset (an id), or, for rows recorded before orders stored their asset,
# one unresolved symbol string.
#
# A key is the asset's current symbol, so every bot whose assets have distinct symbols reads exactly as
# before. Where two holdings would share a key, EVERY one of them takes a suffix — its asset id, or `?` for
# an unresolved string — repeated until all keys differ: `POR#12` and `POR#57` for two assets called POR.
# A bare shared symbol is therefore never a key, which is what lets every boundary refuse one.
module Bot::Composition::HoldingKeys
  module_function

  # candidates: { identity => candidate key }, an identity being an asset id (Integer) or an unresolved
  # string. Returns { identity => key }, every key distinct.
  #
  # Terminates: owners of one clash get distinct tags (ids are unique; several unresolved strings are
  # numbered), so a pair that clashed never matches again — keys only grow at the end.
  def call(candidates)
    keys = candidates.dup
    loop do
      clashes = keys.group_by { |_identity, key| key }.values.select(&:many?)
      return keys if clashes.empty?

      clashes.each do |owners|
        unresolved = owners.map(&:first).grep(String).sort
        owners.each do |identity, key|
          tag = if identity.is_a?(Integer) then identity
                elsif unresolved.one? then '?'
                else "?#{unresolved.index(identity) + 1}"
                end
          keys[identity] = "#{key}##{tag}"
        end
      end
    end
  end

  # The candidate key of each asset: its symbol, else its name, else `#<id>`.
  def candidates_for(asset_ids)
    Asset.where(id: asset_ids).pluck(:id, :symbol, :name).to_h do |id, symbol, name|
      [id, symbol.presence || name.presence || "##{id}"]
    end
  end
end
