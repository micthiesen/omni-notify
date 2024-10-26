export default class BetterMap<K, V> extends Map<K, V> {
	public getOrThrow(key: K): V {
		const value = this.get(key);
		if (value === undefined) {
			throw new Error(`Key not found: ${key}`);
		}
		return value;
	}
}
